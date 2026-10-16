//! Binary detection (libgit2 `git_str_is_binary`, SPEC §4.11) and line helpers.

/// libgit2's heuristic over the first 8000 bytes.
pub fn is_binary(data: &[u8]) -> bool {
    let mut b = &data[..data.len().min(8000)];
    // BOM handling: skip UTF-8, binary for UTF-16/32
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        b = &b[3..];
    } else if b.starts_with(&[0xFF, 0xFE])
        || b.starts_with(&[0xFE, 0xFF])
        || b.starts_with(&[0, 0, 0xFE, 0xFF])
    {
        return true;
    }
    let mut printable: u64 = 0;
    let mut nonprintable: u64 = 0;
    for &c in b {
        if (c > 0x1F && c != 127) || c == b'\x08' || c == 0x1B || c == 0x0C {
            printable += 1;
        } else if c == 0 {
            return true;
        } else if !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B) {
            nonprintable += 1;
        }
    }
    (printable >> 7) < nonprintable
}

/// Number of lines stagit's `writeblob` numbers: `\n`-separated, a final unterminated
/// line counts, an empty blob has 0.
pub fn line_count(data: &[u8]) -> usize {
    if data.is_empty() {
        return 0;
    }
    let n = data.iter().filter(|&&c| c == b'\n').count();
    if data.ends_with(b"\n") {
        n
    } else {
        n + 1
    }
}

/// Iterator over lines *including* their trailing `\n` (last one may lack it).
pub fn lines_with_terminator(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = data;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = memchr(b'\n', rest).map(|i| i + 1).unwrap_or(rest.len());
        let (line, r) = rest.split_at(end);
        rest = r;
        Some(line)
    })
}

#[inline]
pub fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&c| c == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_detection() {
        assert!(!is_binary(b""));
        assert!(!is_binary(b"hello\nworld\n"));
        assert!(is_binary(b"hello\0world"));
        assert!(is_binary(&[0xFF, 0xFE, b'h', 0, b'i', 0]));
        assert!(!is_binary(&[0xEF, 0xBB, 0xBF, b'h', b'i']));
        // PNG header: many non-printables, few printables
        assert!(is_binary(&[
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00
        ]));
        // a few control characters in a long text are tolerated
        let mut text = vec![b'a'; 1000];
        text[10] = 0x01;
        assert!(!is_binary(&text));
        // but many are not: 1 printable per 128 nonprintables tips it
        let mut bad = vec![0x01u8; 10];
        bad.push(b'a');
        assert!(is_binary(&bad));
        // NUL beyond 8000 bytes is not seen
        let mut long = vec![b'a'; 8001];
        long[8000] = 0;
        assert!(!is_binary(&long));
    }

    #[test]
    fn lines() {
        assert_eq!(line_count(b""), 0);
        assert_eq!(line_count(b"a"), 1);
        assert_eq!(line_count(b"a\n"), 1);
        assert_eq!(line_count(b"a\nb"), 2);
        assert_eq!(line_count(b"\n\n"), 2);
        let v: Vec<&[u8]> = lines_with_terminator(b"a\nb").collect();
        assert_eq!(v, vec![&b"a\n"[..], &b"b"[..]]);
    }
}
