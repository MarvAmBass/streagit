//! URL -> bare repository + page path (SPEC §5.1, §5.2, §5.4).

use std::path::{Path, PathBuf};

use crate::git::repo::RepoRef;

pub const MAX_PATH_LEN: usize = 4096;
pub const MAX_SEGMENTS: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub enum PathError {
    /// Malformed or unsafe path -> 404.
    Bad,
    /// Path too long -> 413.
    TooLong,
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Percent-decode one segment; rejects empty, `.`, `..`, NUL, `\` and an encoded `/`.
pub fn decode_segment(seg: &str) -> Result<Vec<u8>, PathError> {
    let b = seg.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|&c| hex(c)),
                b.get(i + 2).and_then(|&c| hex(c)),
            ) else {
                return Err(PathError::Bad);
            };
            out.push(h << 4 | l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    if out.is_empty()
        || out == b"."
        || out == b".."
        || out.iter().any(|&c| c == 0 || c == b'\\' || c == b'/')
    {
        return Err(PathError::Bad);
    }
    Ok(out)
}

/// Split a request path (already without the prefix) into decoded segments.
/// A trailing `/` yields a final empty marker handled by the caller via `trailing_slash`.
pub fn split_path(path: &str) -> Result<(Vec<Vec<u8>>, bool), PathError> {
    if path.len() > MAX_PATH_LEN {
        return Err(PathError::TooLong);
    }
    let trailing = path.ends_with('/');
    let mut segs = Vec::new();
    for seg in path.split('/') {
        if seg.is_empty() {
            continue; // leading slash / trailing slash / double slash (tolerated, collapsed)
        }
        segs.push(decode_segment(seg)?);
        if segs.len() > MAX_SEGMENTS {
            return Err(PathError::TooLong);
        }
    }
    Ok((segs, trailing))
}

fn is_safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'~' | b'+' | b'-'))
}

fn is_version(s: &str) -> bool {
    s.strip_prefix('v').is_some_and(|n| {
        !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()) && !n.starts_with('0')
    })
}

/// `v<digits>` (any digits): never accepted as a unit name, so `html.v0` is not `v0.git`.
fn looks_like_version(s: &str) -> bool {
    s.strip_prefix('v')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// If `seg` is an html-dir segment, the git dir name it maps to.
/// `html` -> `repo.git`, `html.v2` -> `repo.v2.git`, `html.wiki` -> `wiki.git`, `html.wiki.v1` -> `wiki.v1.git`.
pub fn html_to_gitdir(seg: &str) -> Option<String> {
    let rest = seg.strip_prefix("html")?;
    if rest.is_empty() {
        return Some("repo.git".to_string());
    }
    let rest = rest.strip_prefix('.')?;
    let parts: Vec<&str> = rest.split('.').collect();
    match parts.as_slice() {
        [v] if is_version(v) => Some(format!("repo.{v}.git")),
        [unit] if is_safe_name(unit) && !looks_like_version(unit) => Some(format!("{unit}.git")),
        [unit, v] if is_safe_name(unit) && !looks_like_version(unit) && is_version(v) => {
            Some(format!("{unit}.{v}.git"))
        }
        _ => None,
    }
}

/// A segment that names a git directory directly (`repo.git`).
pub fn is_git_dir_name(seg: &str) -> bool {
    seg.strip_suffix(".git")
        .is_some_and(|n| is_safe_name(n) && n != "." && n != "..")
}

#[derive(Debug)]
pub struct Resolved {
    pub repo: RepoRef,
    /// Decoded page path segments after the html/git dir segment.
    pub page: Vec<Vec<u8>>,
    /// Request path up to and including the html segment, re-encoded for redirects.
    pub base: String,
}

/// Whether `dir` looks like a bare repository: a `HEAD` file plus `objects/`
/// and `refs/` directories (symlinks followed).
pub fn is_bare_repo(dir: &Path) -> bool {
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

/// A segment that may name a suffix-less bare repository directory.
fn is_bare_dir_name(seg: &str) -> bool {
    is_safe_name(seg) && !seg.starts_with('.') && !seg.ends_with(".git")
}

/// Resolve decoded segments against `root`. `raw_segments` are the original
/// (encoded) segments, used to build redirect targets.
///
/// Candidates are tried right to left (the longest slot wins); per segment an
/// `html*` name maps to its git dir, `<name>.git` names one directly (with
/// `allow_git_paths`) and, with `allow_bare_dirs`, any other plain name that
/// is itself a bare repository (and not inside one) does too.
pub fn resolve(
    root: &Path,
    canonical_root: &Path,
    segs: &[Vec<u8>],
    raw_segments: &[&str],
    allow_git_paths: bool,
    allow_bare_dirs: bool,
) -> Option<Resolved> {
    let strs: Vec<Option<&str>> = segs.iter().map(|s| std::str::from_utf8(s).ok()).collect();
    for i in (0..segs.len()).rev() {
        let Some(seg) = strs[i] else { continue };
        let Some(prefix) = strs[..i].iter().copied().collect::<Option<Vec<&str>>>() else {
            continue;
        };
        let slot_rel = prefix.join("/");
        let mut slot = PathBuf::from(root);
        slot.extend(&prefix);
        let mut candidates: Vec<(String, bool)> = Vec::with_capacity(2);
        if let Some(g) = html_to_gitdir(seg) {
            candidates.push((g, false));
        }
        if allow_git_paths && is_git_dir_name(seg) {
            candidates.push((seg.to_string(), false));
        }
        if allow_bare_dirs && is_bare_dir_name(seg) {
            candidates.push((seg.to_string(), true));
        }
        for (gitdir_name, bare_dir) in candidates {
            let git_dir = slot.join(&gitdir_name);
            if bare_dir {
                // structure check, and never a directory inside another git dir
                if !is_bare_repo(&git_dir) {
                    continue;
                }
                let mut anc = PathBuf::from(root);
                let mut nested = false;
                for p in &prefix {
                    anc.push(p);
                    if is_bare_repo(&anc) {
                        nested = true;
                        break;
                    }
                }
                if nested {
                    continue;
                }
            } else if !git_dir.join("HEAD").is_file() {
                continue;
            }
            let Ok(canon) = std::fs::canonicalize(&git_dir) else {
                continue;
            };
            if !canon.starts_with(canonical_root) {
                tracing::warn!(path = %git_dir.display(), "repository escapes the root; refused");
                return None;
            }
            let base = raw_segments[..=i]
                .iter()
                .map(|s| format!("/{s}"))
                .collect::<String>();
            let rel = if slot_rel.is_empty() {
                gitdir_name.clone()
            } else {
                format!("{slot_rel}/{gitdir_name}")
            };
            return Some(Resolved {
                repo: RepoRef {
                    git_dir: canon,
                    slot: slot_rel,
                    rel,
                },
                page: segs[i + 1..].to_vec(),
                base,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments() {
        assert_eq!(decode_segment("a%20b").unwrap(), b"a b".to_vec());
        assert_eq!(decode_segment("%C3%BC").unwrap(), "ü".as_bytes().to_vec());
        assert_eq!(decode_segment(".."), Err(PathError::Bad));
        assert_eq!(decode_segment("%2e%2e"), Err(PathError::Bad));
        assert_eq!(decode_segment("a%2Fb"), Err(PathError::Bad));
        assert_eq!(decode_segment("a%00"), Err(PathError::Bad));
        assert_eq!(decode_segment("a%zz"), Err(PathError::Bad));
        assert_eq!(decode_segment("a\\b"), Err(PathError::Bad));
        assert_eq!(decode_segment("."), Err(PathError::Bad));
        let (s, t) = split_path("/a/b/").unwrap();
        assert_eq!(s, vec![b"a".to_vec(), b"b".to_vec()]);
        assert!(t);
        assert_eq!(split_path(&"a/".repeat(100)), Err(PathError::TooLong));
    }

    #[test]
    fn html_segments() {
        assert_eq!(html_to_gitdir("html").as_deref(), Some("repo.git"));
        assert_eq!(html_to_gitdir("html.v2").as_deref(), Some("repo.v2.git"));
        assert_eq!(html_to_gitdir("html.wiki").as_deref(), Some("wiki.git"));
        assert_eq!(
            html_to_gitdir("html.wiki.v1").as_deref(),
            Some("wiki.v1.git")
        );
        assert_eq!(html_to_gitdir("html.v0"), None);
        assert_eq!(html_to_gitdir("html.v01"), None);
        assert_eq!(html_to_gitdir("html."), None);
        assert_eq!(html_to_gitdir("html..v1"), None);
        assert_eq!(html_to_gitdir("htmlx"), None);
        assert_eq!(html_to_gitdir("html.a.b.c"), None);
        assert!(is_git_dir_name("repo.git"));
        assert!(is_git_dir_name("repo.v1.git"));
        assert!(!is_git_dir_name(".git"));
        assert!(!is_git_dir_name("..git"));
        assert!(!is_git_dir_name("a b.git"));
    }

    #[test]
    fn resolution_prefers_longest_slot_and_rejects_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let mk = |p: &str| {
            let d = root.join(p);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        };
        mk("gh/a/repos/b/repo.git");
        mk("gh/a/repos/b/repo.v2.git");
        mk("gh/a/repos/b/wiki.git");
        mk("gh/a/repos/b/forks/x/repo.git");
        mk("gh/a/repos/html/repo.git");
        // a symlink inside the root is fine, one escaping it is not
        let outside = dir.path().join("outside/repo.git");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("HEAD"), "x").unwrap();
        std::fs::create_dir_all(root.join("gh/u/stars/a")).unwrap();
        std::os::unix::fs::symlink("../../../a/repos/b", root.join("gh/u/stars/a/b")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("gh/evil.git")).unwrap();
        let croot = std::fs::canonicalize(&root).unwrap();

        let res = |p: &str| {
            let (segs, _) = split_path(p).unwrap();
            let raw: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
            resolve(&root, &croot, &segs, &raw, true, false)
        };
        let r = res("/gh/a/repos/b/html/log.html").unwrap();
        assert!(r.repo.git_dir.ends_with("gh/a/repos/b/repo.git"));
        assert_eq!(r.repo.slot, "gh/a/repos/b");
        assert_eq!(r.page, vec![b"log.html".to_vec()]);
        assert_eq!(r.base, "/gh/a/repos/b/html");
        assert!(res("/gh/a/repos/b/html.v2/files.html")
            .unwrap()
            .repo
            .git_dir
            .ends_with("repo.v2.git"));
        assert!(res("/gh/a/repos/b/html.v3/files.html").is_none());
        assert!(res("/gh/a/repos/b/html.wiki/log.html")
            .unwrap()
            .repo
            .git_dir
            .ends_with("wiki.git"));
        assert!(res("/gh/a/repos/b/forks/x/html/log.html")
            .unwrap()
            .repo
            .git_dir
            .ends_with("forks/x/repo.git"));
        assert!(res("/gh/a/repos/b/forks/x/repo.git/log.html")
            .unwrap()
            .repo
            .git_dir
            .ends_with("forks/x/repo.git"));
        // a repo named "html": the rightmost html segment wins
        let r = res("/gh/a/repos/html/html/refs.html").unwrap();
        assert!(r.repo.git_dir.ends_with("repos/html/repo.git"));
        assert_eq!(r.page, vec![b"refs.html".to_vec()]);
        // star symlink resolves inside the root
        let r = res("/gh/u/stars/a/b/html/log.html").unwrap();
        assert!(r.repo.git_dir.ends_with("gh/a/repos/b/repo.git"));
        // escaping symlink refused
        assert!(res("/gh/evil.git/log.html").is_none());
        assert!(res("/nothing/html/log.html").is_none());
        // direct git paths can be disabled
        let (segs, _) = split_path("/gh/a/repos/b/repo.git/log.html").unwrap();
        let raw: Vec<&str> = ["gh", "a", "repos", "b", "repo.git", "log.html"].to_vec();
        assert!(resolve(&root, &croot, &segs, &raw, false, false).is_none());
        assert_eq!(
            res("/gh/a/repos/b/repo.git/log.html").unwrap().repo.rel,
            "gh/a/repos/b/repo.git"
        );
        assert_eq!(
            res("/gh/a/repos/b/html/log.html").unwrap().repo.rel,
            "gh/a/repos/b/repo.git"
        );
    }

    #[test]
    fn bare_dirs_without_git_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let mk = |p: &str| {
            let d = root.join(p);
            std::fs::create_dir_all(d.join("objects")).unwrap();
            std::fs::create_dir_all(d.join("refs")).unwrap();
            std::fs::write(d.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        };
        mk("a/b.git");
        mk("a/c");
        mk("a/c/modules/sub"); // a git dir inside a git dir is never a repository
        mk("top");
        // HEAD alone is not enough for a suffix-less directory
        std::fs::create_dir_all(root.join("a/notrepo")).unwrap();
        std::fs::write(root.join("a/notrepo/HEAD"), "x").unwrap();
        let outside = dir.path().join("outside");
        mk("../outside");
        std::os::unix::fs::symlink(&outside, root.join("a/evil")).unwrap();
        let croot = std::fs::canonicalize(&root).unwrap();
        let res = |p: &str, bare: bool| {
            let (segs, _) = split_path(p).unwrap();
            let raw: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
            resolve(&root, &croot, &segs, &raw, true, bare)
        };
        assert!(res("/a/c/log.html", false).is_none());
        let r = res("/a/c/log.html", true).unwrap();
        assert!(r.repo.git_dir.ends_with("a/c"));
        assert_eq!((r.repo.slot.as_str(), r.repo.rel.as_str()), ("a", "a/c"));
        assert_eq!(r.base, "/a/c");
        let r = res("/a/c/file/x/y.html", true).unwrap();
        assert!(r.repo.git_dir.ends_with("a/c"));
        assert_eq!(r.page.len(), 3);
        let r = res("/a/b.git/log.html", true).unwrap();
        assert_eq!(r.repo.rel, "a/b.git");
        assert_eq!(res("/top/refs.html", true).unwrap().repo.rel, "top");
        assert!(res("/a/c/modules/sub/log.html", true)
            .unwrap()
            .repo
            .git_dir
            .ends_with("a/c"));
        assert!(res("/a/notrepo/log.html", true).is_none());
        assert!(res("/a/evil/log.html", true).is_none());
        assert!(split_path("/a/%2e%2e/outside/log.html").is_err());
        assert!(res("/a/log.html", true).is_none());
    }
}
