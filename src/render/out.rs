//! Output sink and stagit's escaping functions (SPEC §4.1, §7.1).

use std::fmt;

/// Why a render stopped early.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The client went away; nothing more to do.
    #[error("client disconnected")]
    Cancelled,
    /// The render deadline passed; a notice has to be written by the caller.
    #[error("render time limit exceeded")]
    Timeout,
    /// The client did not read the stream before the deadline; nothing more is sent.
    #[error("client too slow")]
    SlowClient,
    /// Something in the repository could not be read.
    #[error("{0}")]
    Git(#[from] anyhow::Error),
}

pub type R = Result<(), RenderError>;

impl From<gix::Error> for RenderError {
    fn from(e: gix::Error) -> Self {
        RenderError::Git(anyhow::Error::from(e))
    }
}

/// Where rendered bytes go.
pub trait Out {
    fn write(&mut self, b: &[u8]) -> R;
    /// Hand off whatever is buffered (the first flush shows the page skeleton early).
    fn flush(&mut self) -> R;

    /// stagit's `xmlencode`.
    fn esc(&mut self, b: &[u8]) -> R {
        let mut start = 0;
        for (i, &c) in b.iter().enumerate() {
            let rep: &[u8] = match c {
                b'<' => b"&lt;",
                b'>' => b"&gt;",
                b'\'' => b"&#39;",
                b'&' => b"&amp;",
                b'"' => b"&quot;",
                _ => continue,
            };
            if start < i {
                self.write(&b[start..i])?;
            }
            self.write(rep)?;
            start = i + 1;
        }
        if start < b.len() {
            self.write(&b[start..])?;
        }
        Ok(())
    }

    /// stagit's `xmlencodeline`: like `esc` but `\r` and `\n` are dropped.
    fn escl(&mut self, b: &[u8]) -> R {
        let mut start = 0;
        for (i, &c) in b.iter().enumerate() {
            let rep: &[u8] = match c {
                b'<' => b"&lt;",
                b'>' => b"&gt;",
                b'\'' => b"&#39;",
                b'&' => b"&amp;",
                b'"' => b"&quot;",
                b'\r' | b'\n' => b"",
                _ => continue,
            };
            if start < i {
                self.write(&b[start..i])?;
            }
            self.write(rep)?;
            start = i + 1;
        }
        if start < b.len() {
            self.write(&b[start..])?;
        }
        Ok(())
    }

    /// stagit's `percentencode`.
    fn pct(&mut self, b: &[u8]) -> R {
        let mut start = 0;
        for (i, &c) in b.iter().enumerate() {
            if !pct_needs_encoding(c) {
                continue;
            }
            if start < i {
                self.write(&b[start..i])?;
            }
            let hex = b"0123456789ABCDEF";
            self.write(&[b'%', hex[(c >> 4) as usize], hex[(c & 0xf) as usize]])?;
            start = i + 1;
        }
        if start < b.len() {
            self.write(&b[start..])?;
        }
        Ok(())
    }

    fn fmt(&mut self, args: fmt::Arguments<'_>) -> R {
        let mut s = String::new();
        fmt::write(&mut s, args).expect("formatting never fails");
        self.write(s.as_bytes())
    }
}

/// stagit's rule: encode if `c < ','`, `c >= 127`, `':' <= c <= '@'`, `'['`, `']'`.
#[inline]
pub fn pct_needs_encoding(c: u8) -> bool {
    !(b','..127).contains(&c) || (b':'..=b'@').contains(&c) || c == b'[' || c == b']'
}

/// Escape into a fresh `Vec` (for titles and attribute values).
pub fn esc_vec(b: &[u8]) -> Vec<u8> {
    let mut v = VecOut::default();
    v.esc(b).expect("vec never fails");
    v.0
}

/// A plain in-memory sink (tests, cache fills).
#[derive(Default, Debug)]
pub struct VecOut(pub Vec<u8>);

impl Out for VecOut {
    fn write(&mut self, b: &[u8]) -> R {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn flush(&mut self) -> R {
        Ok(())
    }
}

/// Convenience macro: `w!(out, "literal {}", x)` formats into the sink.
#[macro_export]
macro_rules! w {
    ($out:expr, $($arg:tt)*) => {
        $out.fmt(format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn esc_s(s: &[u8]) -> String {
        String::from_utf8(esc_vec(s)).unwrap()
    }

    fn pct_s(s: &[u8]) -> String {
        let mut v = VecOut::default();
        v.pct(s).unwrap();
        String::from_utf8(v.0).unwrap()
    }

    #[test]
    fn xmlencode_table() {
        assert_eq!(
            esc_s(b"<a href=\"x\">it's & more</a>"),
            "&lt;a href=&quot;x&quot;&gt;it&#39;s &amp; more&lt;/a&gt;"
        );
        assert_eq!(esc_s(b"plain"), "plain");
    }

    #[test]
    fn xmlencode_keeps_bytes() {
        // control characters and invalid UTF-8 are copied verbatim
        assert_eq!(esc_vec(b"\x01\x7f\xff\r\n"), b"\x01\x7f\xff\r\n".to_vec());
        let mut v = VecOut::default();
        v.escl(b"a\r\nb<").unwrap();
        assert_eq!(v.0, b"ab&lt;".to_vec());
    }

    #[test]
    #[allow(clippy::manual_range_contains)]
    fn percentencode_table() {
        // reference implementation straight from stagit.c
        fn reference(b: &[u8]) -> String {
            let mut s = String::new();
            for &c in b {
                if c < b',' || c >= 127 || (c >= b':' && c <= b'@') || c == b'[' || c == b']' {
                    s.push_str(&format!("%{:02X}", c));
                } else {
                    s.push(c as char);
                }
            }
            s
        }
        let all: Vec<u8> = (0u8..=255).collect();
        assert_eq!(pct_s(&all), reference(&all));
        assert_eq!(
            pct_s(b"a b#c%d+e?f/g.h,i-j_k~l"),
            "a%20b%23c%25d%2Be%3Ff/g.h,i-j_k~l"
        );
        assert_eq!(pct_s("ü".as_bytes()), "%C3%BC");
    }
}
