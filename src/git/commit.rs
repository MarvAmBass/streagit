//! Commit metadata the way libgit2/stagit sees it.

use gix::ObjectId;

/// A git signature: name, email, time (seconds) and offset (minutes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sig {
    pub name: Vec<u8>,
    pub email: Vec<u8>,
    pub time: i64,
    pub offset_min: i32,
}

impl Sig {
    pub fn from_ref(s: &gix::actor::SignatureRef<'_>) -> Sig {
        let (time, offset_min) = parse_time(s.time.as_ref());
        Sig {
            name: s.name.to_vec(),
            email: s.email.to_vec(),
            time,
            offset_min,
        }
    }
}

/// Parse `<seconds> <+|-HHMM>` like libgit2 (`git_signature__parse`): missing or
/// malformed parts default to 0.
pub fn parse_time(raw: &[u8]) -> (i64, i32) {
    let s = String::from_utf8_lossy(raw);
    let mut it = s.split_ascii_whitespace();
    let secs = it.next().and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
    let off = it
        .next()
        .and_then(|o| {
            let (sign, digits) = match o.as_bytes().first() {
                Some(b'-') => (-1, &o[1..]),
                Some(b'+') => (1, &o[1..]),
                _ => (1, o),
            };
            if digits.len() < 4 || !digits.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let h: i32 = digits[..2].parse().ok()?;
            let m: i32 = digits[2..4].parse().ok()?;
            Some(sign * (h * 60 + m))
        })
        .unwrap_or(0);
    (secs, off)
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub id: ObjectId,
    pub tree: ObjectId,
    pub parent: Option<ObjectId>,
    pub author: Option<Sig>,
    pub committer: Option<Sig>,
    /// `git_commit_summary` (may be empty; libgit2 never returns NULL for it).
    pub summary: Vec<u8>,
    /// `git_commit_message`: raw message with leading newlines trimmed.
    pub message: Vec<u8>,
}

impl CommitInfo {
    pub fn from_commit(c: &gix::Commit<'_>) -> anyhow::Result<CommitInfo> {
        let r = c.decode()?;
        let message = libgit2_message(r.message);
        let summary = summary(&message);
        let parent = r.parents().next();
        let info = CommitInfo {
            id: c.id,
            tree: r.tree(),
            parent,
            author: r.author().ok().map(|a| Sig::from_ref(&a)),
            committer: r.committer().ok().map(|a| Sig::from_ref(&a)),
            summary,
            message,
        };
        Ok(info)
    }
}

/// `git_commit_message`: leading `\n` are dropped.
pub fn libgit2_message(raw: &[u8]) -> Vec<u8> {
    let start = raw.iter().position(|&c| c != b'\n').unwrap_or(raw.len());
    raw[start..].to_vec()
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
}

/// `git_commit_summary`: the first paragraph with whitespace runs containing a
/// newline collapsed to one space; trailing whitespace dropped.
pub fn summary(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut space: Option<usize> = None;
    let mut space_has_nl = false;
    let mut i = 0;
    while i < msg.len() {
        let c = msg[i];
        if c == b'\n' {
            match msg.get(i + 1) {
                None | Some(b'\n') => break,
                _ => {
                    // stop if the next line contains only whitespace
                    let mut j = i + 1;
                    while j < msg.len() && is_space(msg[j]) && msg[j] != b'\n' {
                        j += 1;
                    }
                    if j >= msg.len() || msg[j] == b'\n' {
                        break;
                    }
                }
            }
        }
        if is_space(c) {
            if space.is_none() {
                space = Some(i);
                space_has_nl = false;
            }
            space_has_nl |= c == b'\n';
        } else {
            if let Some(s) = space.take() {
                if space_has_nl {
                    out.push(b' ');
                } else {
                    out.extend_from_slice(&msg[s..i]);
                }
            }
            out.push(c);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries() {
        assert_eq!(summary(b"Subject line\n\nBody\n"), b"Subject line".to_vec());
        assert_eq!(
            summary(b"Subject\ncontinued  here\n\nBody"),
            b"Subject continued  here".to_vec()
        );
        assert_eq!(summary(b"Subject   \n"), b"Subject".to_vec());
        assert_eq!(summary(b"a\n  \nb"), b"a".to_vec());
        assert_eq!(summary(b""), b"".to_vec());
        assert_eq!(summary(b"\n\nx"), b"".to_vec());
        assert_eq!(libgit2_message(b"\n\nx\n"), b"x\n".to_vec());
    }

    #[test]
    fn times() {
        assert_eq!(parse_time(b"1699570671 -0500"), (1_699_570_671, -300));
        assert_eq!(parse_time(b"1699570671 +0530"), (1_699_570_671, 330));
        assert_eq!(parse_time(b"1699570671"), (1_699_570_671, 0));
        assert_eq!(parse_time(b"garbage"), (0, 0));
    }
}
