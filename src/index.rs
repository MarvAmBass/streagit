//! `--index`: an overview of all repositories below the root, in the format
//! of stagit 1.3's `stagit-index`.
//!
//! The root is walked at most `--index-depth` directory levels deep. A
//! directory is a repository when it has a `HEAD` file and `objects/` and
//! `refs/` directories; repositories are not descended into, nor are hidden
//! directories (`.name`). Symlinked directories are followed when they stay
//! inside the root; every repository is listed once, under its canonical path.
//! Only repositories that the URL resolver serves are listed, so every link
//! works. The list is newest first by the author time of HEAD's commit
//! (repositories without commits last), then by path.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::exclude::Excludes;
use crate::git::repo::{open_repo, read_first_line, resolve_head};
use crate::http::resolve::{is_bare_repo, resolve};
use crate::render::out::{Out, VecOut};
use crate::render::time::time_short;

/// One listed repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Canonical path of the git directory below the root (`team/tool.git`).
    pub rel: String,
    pub description: Vec<u8>,
    pub owner: Vec<u8>,
    /// Author time of HEAD's commit.
    pub last_commit: Option<i64>,
}

/// What the walk needs to know.
pub struct Discovery<'a> {
    /// Canonical data root.
    pub root: &'a Path,
    pub depth: usize,
    pub excludes: &'a Excludes,
    pub allow_git_paths: bool,
    pub allow_bare_dirs: bool,
}

impl Discovery<'_> {
    /// Whether the resolver maps `<rel>/log.html` to exactly `canon`.
    fn served(&self, rel: &str, canon: &Path) -> bool {
        let mut raw: Vec<&str> = rel.split('/').collect();
        raw.push("log.html");
        let segs: Vec<Vec<u8>> = raw.iter().map(|s| s.as_bytes().to_vec()).collect();
        resolve(
            self.root,
            self.root,
            &segs,
            &raw,
            self.allow_git_paths,
            self.allow_bare_dirs,
        )
        .is_some_and(|r| r.repo.git_dir == canon && r.page.len() == 1)
    }

    /// Find all repositories (canonical paths below the root, sorted by path).
    pub fn find(&self) -> Vec<(String, PathBuf)> {
        let mut found = Vec::new();
        let mut visited: HashSet<PathBuf> = HashSet::new();
        visited.insert(self.root.to_path_buf());
        // (directory, path below the root as walked, level of its children)
        let mut stack = vec![(self.root.to_path_buf(), String::new(), 1usize)];
        while let Some((dir, walked, level)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.'))
                .collect();
            names.sort();
            // pushed in reverse so the walk visits names in order
            for name in names.into_iter().rev() {
                let child = dir.join(&name);
                let walked_rel = if walked.is_empty() {
                    name.clone()
                } else {
                    format!("{walked}/{name}")
                };
                let Ok(canon) = std::fs::canonicalize(&child) else {
                    continue;
                };
                if !canon.is_dir() {
                    continue;
                }
                let Some(rel) = canon
                    .strip_prefix(self.root)
                    .ok()
                    .and_then(|r| r.to_str())
                    .filter(|r| !r.is_empty())
                    .map(str::to_string)
                else {
                    continue; // outside the root (symlink) or not UTF-8
                };
                if self.excludes.is_excluded(&walked_rel) || self.excludes.is_excluded(&rel) {
                    continue;
                }
                if !visited.insert(canon.clone()) {
                    continue; // symlink loop or second path to the same directory
                }
                if is_bare_repo(&canon) {
                    if self.served(&rel, &canon) {
                        found.push((rel, canon));
                    } else {
                        tracing::debug!(path = rel, "repository not reachable by URL; not listed");
                    }
                } else if level < self.depth {
                    stack.push((canon, walked_rel, level + 1));
                }
            }
        }
        found.sort();
        found
    }
}

/// Walk the root and read each repository's metadata and last commit.
pub fn build(d: &Discovery<'_>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = d
        .find()
        .into_iter()
        .map(|(rel, git_dir)| Entry {
            description: read_first_line(&git_dir.join("description"), 255),
            owner: read_first_line(&git_dir.join("owner"), 255),
            last_commit: last_commit(&git_dir),
            rel,
        })
        .collect();
    entries.sort_by(|a, b| {
        b.last_commit
            .cmp(&a.last_commit)
            .then_with(|| a.rel.cmp(&b.rel))
    });
    entries
}

fn last_commit(git_dir: &Path) -> Option<i64> {
    let repo = match open_repo(git_dir) {
        Ok(r) => r.to_thread_local(),
        Err(e) => {
            tracing::warn!(path = %git_dir.display(), error = format!("{e:#}"), "index: cannot open repository");
            return None;
        }
    };
    let id = resolve_head(&repo).ok().flatten()?;
    let commit = repo.find_commit(id).ok()?;
    let author = commit.author().ok()?;
    Some(crate::git::commit::parse_time(author.time.as_ref()).0)
}

/// Render the index page for the directory `dir` (`""` for the root, else
/// `a/b` without slashes at the ends), or `None` when nothing is listed below
/// a non-root `dir`. Names and links are relative to `dir`.
pub fn render(entries: &[Entry], dir: &str) -> Option<Vec<u8>> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let rows: Vec<(&str, &Entry)> = entries
        .iter()
        .filter_map(|e| e.rel.strip_prefix(prefix.as_str()).map(|r| (r, e)))
        .collect();
    if rows.is_empty() && !dir.is_empty() {
        return None;
    }
    let relpath = "../".repeat(if dir.is_empty() {
        0
    } else {
        dir.split('/').count()
    });
    let title = "Repositories";
    let mut out = VecOut::default();
    let o = &mut out;
    let _ = o.write(b"<!DOCTYPE html>\n<html>\n<head>\n<meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n<title>");
    let _ = o.esc(title.as_bytes());
    let _ = o.fmt(format_args!(
        "</title>\n<link rel=\"icon\" type=\"image/png\" href=\"{relpath}favicon.png\" />\n"
    ));
    let _ = o.fmt(format_args!(
        "<link rel=\"stylesheet\" type=\"text/css\" href=\"{relpath}style.css\" />\n"
    ));
    let _ = o.write(b"</head>\n<body>\n");
    let _ = o.fmt(format_args!(
        "<table>\n<tr><td><img src=\"{relpath}logo.png\" alt=\"\" width=\"32\" height=\"32\" /></td>\n<td><span class=\"desc\">"
    ));
    let _ = o.esc(title.as_bytes());
    let _ = o.write(b"</span></td></tr><tr><td></td><td>\n</td></tr>\n</table>\n<hr/>\n<div id=\"content\">\n<table id=\"index\"><thead>\n<tr><td><b>Name</b></td><td><b>Description</b></td><td><b>Owner</b></td><td><b>Last commit</b></td></tr></thead><tbody>\n");
    for (link, e) in rows {
        let name = link.strip_suffix(".git").unwrap_or(link);
        let _ = o.write(b"<tr><td><a href=\"");
        let _ = o.pct(link.as_bytes());
        let _ = o.write(b"/log.html\">");
        let _ = o.esc(name.as_bytes());
        let _ = o.write(b"</a></td><td>");
        let _ = o.esc(&e.description);
        let _ = o.write(b"</td><td>");
        let _ = o.esc(&e.owner);
        let _ = o.write(b"</td><td>");
        if let Some(t) = e.last_commit {
            let _ = o.write(time_short(t).as_bytes());
        }
        let _ = o.write(b"</td></tr>");
    }
    let _ = o.write(b"</tbody>\n</table>\n</div>\n</body>\n</html>\n");
    Some(out.0)
}

/// The repository list, rebuilt at most every `ttl`.
pub struct IndexCache {
    ttl: Duration,
    inner: tokio::sync::Mutex<Option<(Instant, Arc<Vec<Entry>>)>>,
}

impl IndexCache {
    pub fn new(ttl: Duration) -> IndexCache {
        IndexCache {
            ttl,
            inner: tokio::sync::Mutex::new(None),
        }
    }

    /// The cached list, or a fresh one from `walk` (run on the blocking pool;
    /// concurrent callers wait for the same walk).
    pub async fn get<F>(&self, walk: F) -> Arc<Vec<Entry>>
    where
        F: FnOnce() -> Vec<Entry> + Send + 'static,
    {
        let mut guard = self.inner.lock().await;
        if let Some((built, list)) = guard.as_ref() {
            if built.elapsed() < self.ttl {
                return list.clone();
            }
        }
        let started = Instant::now();
        let list = Arc::new(tokio::task::spawn_blocking(walk).await.unwrap_or_else(|e| {
            tracing::error!(error = %e, "index walk panicked");
            Vec::new()
        }));
        tracing::debug!(
            repos = list.len(),
            ms = started.elapsed().as_millis() as u64,
            "index rebuilt"
        );
        *guard = Some((Instant::now(), list.clone()));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(root: &Path, p: &str) {
        let d = root.join(p);
        std::fs::create_dir_all(d.join("objects")).unwrap();
        std::fs::create_dir_all(d.join("refs")).unwrap();
        std::fs::write(d.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    #[test]
    fn walk_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        for p in [
            "top.git",
            "a/b.git",
            "a/c",
            "a/c/modules/inner", // inside a repository: not listed
            "x/y/z.git",         // depth 3
            "x/y/w/deep.git",    // depth 4: beyond the default
            "mirrors/m.git",     // excluded
            ".hidden/h.git",
            "a/bad name.git", // not a URL-safe name
        ] {
            mk(&root, p);
        }
        std::fs::create_dir_all(root.join("loop")).unwrap();
        std::os::unix::fs::symlink("..", root.join("loop/up")).unwrap();
        std::os::unix::fs::symlink("../a", root.join("loop/a2")).unwrap();
        let outside = dir.path().join("outside");
        mk(dir.path(), "outside/o.git");
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let croot = std::fs::canonicalize(&root).unwrap();
        let ex = Excludes::new(&["mirrors".into()]);
        let d = Discovery {
            root: &croot,
            depth: 3,
            excludes: &ex,
            allow_git_paths: true,
            allow_bare_dirs: true,
        };
        let rels: Vec<String> = d.find().into_iter().map(|(r, _)| r).collect();
        assert_eq!(rels, ["a/b.git", "a/c", "top.git", "x/y/z.git"]);
        // without suffix-less directories `a/c` is not reachable, so not listed
        let d = Discovery {
            allow_bare_dirs: false,
            ..d
        };
        let rels: Vec<String> = d.find().into_iter().map(|(r, _)| r).collect();
        assert_eq!(rels, ["a/b.git", "top.git", "x/y/z.git"]);
        let d = Discovery { depth: 4, ..d };
        assert_eq!(d.find().len(), 4);
        let d = Discovery { depth: 1, ..d };
        let rels: Vec<String> = d.find().into_iter().map(|(r, _)| r).collect();
        assert_eq!(rels, ["top.git"]);
    }

    #[test]
    fn rendering() {
        let entries = vec![
            Entry {
                rel: "a/b.git".into(),
                description: b"<b> & 'c'".to_vec(),
                owner: b"Alice".to_vec(),
                last_commit: Some(1_700_000_000),
            },
            Entry {
                rel: "a/c++".into(),
                description: Vec::new(),
                owner: Vec::new(),
                last_commit: None,
            },
            Entry {
                rel: "top.git".into(),
                description: Vec::new(),
                owner: Vec::new(),
                last_commit: None,
            },
        ];
        let page = String::from_utf8(render(&entries, "").unwrap()).unwrap();
        assert!(page.contains("<tr><td><a href=\"a/b.git/log.html\">a/b</a></td><td>&lt;b&gt; &amp; &#39;c&#39;</td><td>Alice</td><td>2023-11-14 22:13</td></tr>"));
        assert!(page.contains("<a href=\"a/c%2B%2B/log.html\">a/c++</a>"));
        assert!(page.contains("href=\"style.css\""));
        let sub = String::from_utf8(render(&entries, "a").unwrap()).unwrap();
        assert!(sub.contains("<a href=\"b.git/log.html\">b</a>"));
        assert!(sub.contains("<img src=\"../logo.png\""));
        assert!(!sub.contains("top"));
        assert!(render(&entries, "nothing").is_none());
        assert!(render(&[], "").is_some());
    }
}
