//! `--exclude`: paths below the root that are neither listed nor served.
//!
//! A pattern is a `/`-separated path relative to the root. Each segment may
//! use `*` (any run of characters within the segment), `?` (one character)
//! and a whole segment `**` matches any number of segments. A path is
//! excluded when the pattern matches the path or any of its parent
//! directories, so `team` excludes `team/` and everything below it and
//! `*/private` excludes `x/private/...` for every `x`.

/// The parsed `--exclude` patterns.
#[derive(Debug, Clone, Default)]
pub struct Excludes {
    patterns: Vec<Vec<String>>,
}

fn segments(p: &str) -> Vec<&str> {
    p.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect()
}

/// `*` / `?` glob for one path segment.
fn glob(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| glob(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && glob(rest, &s[1..]),
        Some((c, rest)) => s.first() == Some(c) && glob(rest, &s[1..]),
    }
}

/// Whether `pat` matches a prefix of `path` (at segment boundaries).
fn matches_prefix(pat: &[String], path: &[&str]) -> bool {
    match pat.split_first() {
        None => true,
        Some((p, rest)) if p == "**" => (0..=path.len()).any(|i| matches_prefix(rest, &path[i..])),
        Some((p, rest)) => match path.split_first() {
            Some((s, tail)) => glob(p.as_bytes(), s.as_bytes()) && matches_prefix(rest, tail),
            None => false,
        },
    }
}

impl Excludes {
    pub fn new(patterns: &[String]) -> Excludes {
        Excludes {
            patterns: patterns
                .iter()
                .map(|p| segments(p.trim()).into_iter().map(str::to_string).collect())
                .filter(|p: &Vec<String>| !p.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether the root-relative path `rel` (`a/b.git`) is excluded.
    pub fn is_excluded(&self, rel: &str) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let path = segments(rel);
        self.patterns.iter().any(|p| matches_prefix(p, &path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        let e = Excludes::new(&[
            "mirrors".into(),
            "team/secret.git".into(),
            "*/private".into(),
            "**/tmp-*".into(),
            " ".into(),
        ]);
        assert!(e.is_excluded("mirrors"));
        assert!(e.is_excluded("mirrors/x/y.git"));
        assert!(!e.is_excluded("mirrors2/y.git"));
        assert!(!e.is_excluded("other/mirrors"));
        assert!(e.is_excluded("team/secret.git"));
        assert!(!e.is_excluded("team/secret"));
        assert!(!e.is_excluded("team/public.git"));
        assert!(e.is_excluded("x/private/a.git"));
        assert!(!e.is_excluded("private/a.git"));
        assert!(e.is_excluded("a/b/tmp-1/c.git"));
        assert!(e.is_excluded("tmp-2"));
        assert!(!e.is_excluded("a/b.git"));
        assert!(!e.is_excluded(""));
        assert!(Excludes::new(&[]).is_empty());
        let q = Excludes::new(&["a?c/".into()]);
        assert!(q.is_excluded("abc/d"));
        assert!(!q.is_excluded("abbc/d"));
    }
}
