//! Repository handles, metadata files and HEAD resolution (SPEC §4.9, §6.1, §8.5).

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use gix::ObjectId;
use lru::LruCache;

use crate::config::NameFrom;

/// A resolved repository location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    /// Canonical path of the bare git directory.
    pub git_dir: PathBuf,
    /// Slot path relative to the root (`github.com/a/repos/b`), `""` for direct `.git` paths at the root.
    pub slot: String,
}

/// Metadata shown in the page header.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub name: String,
    pub description: Vec<u8>,
    pub url: Vec<u8>,
    pub owner: Vec<u8>,
    pub readme: Option<&'static str>,
    pub license: Option<&'static str>,
    pub submodules: bool,
}

/// Bounded cache of open repositories, keyed by canonical git dir.
pub struct RepoCache {
    inner: Mutex<LruCache<PathBuf, Arc<gix::ThreadSafeRepository>>>,
    object_cache_bytes: usize,
}

impl RepoCache {
    pub fn new(capacity: usize, object_cache_bytes: usize) -> RepoCache {
        RepoCache {
            inner: Mutex::new(LruCache::new(NonZeroUsize::new(capacity.max(1)).unwrap())),
            object_cache_bytes,
        }
    }

    /// Open (or reuse) the repository at `git_dir` and return a thread-local handle.
    pub fn open(&self, git_dir: &Path) -> anyhow::Result<gix::Repository> {
        let shared = {
            let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            guard.get(git_dir).cloned()
        };
        let shared = match shared {
            Some(r) => r,
            None => {
                let opened =
                    gix::ThreadSafeRepository::open_opts(git_dir, gix::open::Options::isolated())
                        .with_context(|| format!("open {}", git_dir.display()))?;
                let arc = Arc::new(opened);
                let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                guard.put(git_dir.to_path_buf(), arc.clone());
                arc
            }
        };
        let mut repo = shared.to_thread_local();
        repo.object_cache_size_if_unset(self.object_cache_bytes);
        Ok(repo)
    }

    /// Drop a handle (after an error that suggests the repo moved away).
    pub fn evict(&self, git_dir: &Path) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.pop(git_dir);
    }
}

fn read_first_line(path: &Path, max: usize) -> Vec<u8> {
    match std::fs::read(path) {
        Ok(data) => {
            let end = data.iter().position(|&c| c == b'\n').unwrap_or(data.len());
            let mut line = data[..end.min(max)].to_vec();
            while line
                .last()
                .is_some_and(|c| matches!(c, b'\r' | b' ' | b'\t'))
            {
                line.pop();
            }
            line
        }
        Err(_) => Vec::new(),
    }
}

/// Read `description`, `url`, `owner` and compute the display name.
pub fn read_meta(r: &RepoRef, name_from: NameFrom) -> Meta {
    let description = read_first_line(&r.git_dir.join("description"), 255);
    let url = read_first_line(&r.git_dir.join("url"), 1023);
    let owner = read_first_line(&r.git_dir.join("owner"), 255);
    let dir_name = r
        .git_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir_name = dir_name
        .strip_suffix(".git")
        .map(str::to_string)
        .unwrap_or(dir_name);
    let slot_name = r
        .slot
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let name = match name_from {
        NameFrom::Dir => dir_name,
        NameFrom::Slot => slot_name.unwrap_or(dir_name),
        NameFrom::OwnerSlot => {
            let slot = slot_name.unwrap_or(dir_name);
            let owner_s = if !owner.is_empty() {
                String::from_utf8_lossy(&owner).into_owned()
            } else {
                // <host>/<owner>/repos/<name> -> owner is two levels up the slot
                let parts: Vec<&str> = r.slot.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() >= 3 {
                    parts[parts.len() - 3].to_string()
                } else {
                    String::new()
                }
            };
            if owner_s.is_empty() {
                slot
            } else {
                format!("{owner_s}/{slot}")
            }
        }
    };
    Meta {
        name,
        description,
        url,
        owner,
        readme: None,
        license: None,
        submodules: false,
    }
}

/// Resolve what stagit calls HEAD: the checked-out branch, falling back to
/// `main`, `master`, then the first branch by name. `None` for an empty repo.
pub fn resolve_head(repo: &gix::Repository) -> anyhow::Result<Option<ObjectId>> {
    if let Ok(mut head) = repo.head() {
        if let Ok(Some(id)) = head.try_peel_to_id() {
            if is_commit(repo, id.detach()) {
                return Ok(Some(id.detach()));
            }
        }
    }
    for name in ["refs/heads/main", "refs/heads/master"] {
        if let Some(id) = peel_ref(repo, name)? {
            return Ok(Some(id));
        }
    }
    let platform = repo.references()?;
    let mut names: Vec<String> = Vec::new();
    for r in platform.local_branches()? {
        let r = r?;
        names.push(r.name().as_bstr().to_string());
    }
    names.sort();
    for name in names {
        if let Some(id) = peel_ref(repo, name.as_str())? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

/// Peel a full ref name to a commit id, `None` if it does not exist or is not a commit.
pub fn peel_ref(repo: &gix::Repository, name: &str) -> anyhow::Result<Option<ObjectId>> {
    let Some(mut r) = repo.try_find_reference(name)? else {
        return Ok(None);
    };
    match r.peel_to_id() {
        Ok(id) if is_commit(repo, id.detach()) => Ok(Some(id.detach())),
        _ => Ok(None),
    }
}

/// Resolve a shorthand (`main`, `v1.0`) under `refs/heads/` then `refs/tags/`.
pub fn resolve_shorthand(repo: &gix::Repository, short: &str) -> anyhow::Result<Option<ObjectId>> {
    if short.is_empty() || short.contains("..") || short.starts_with('/') || short.contains('\0') {
        return Ok(None);
    }
    for prefix in ["refs/heads/", "refs/tags/"] {
        if let Some(id) = peel_ref(repo, &format!("{prefix}{short}"))? {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

pub fn is_commit(repo: &gix::Repository, id: ObjectId) -> bool {
    matches!(repo.try_find_header(id), Ok(Some(h)) if h.kind() == gix::object::Kind::Commit)
}

/// Fill `readme`/`license`/`submodules` from HEAD's tree (SPEC §4.10).
pub fn detect_links(
    repo: &gix::Repository,
    tree_id: ObjectId,
    meta: &mut Meta,
) -> anyhow::Result<()> {
    let tree = repo.find_tree(tree_id)?;
    let mut names: Vec<(Vec<u8>, bool)> = Vec::new();
    for e in tree.iter() {
        let e = e?;
        names.push((e.filename().to_vec(), e.mode().is_blob_or_symlink()));
    }
    let has = |n: &str| {
        names
            .iter()
            .any(|(name, blob)| *blob && name == n.as_bytes())
    };
    meta.license = ["LICENSE", "LICENSE.md", "LICENSE.txt", "COPYING"]
        .into_iter()
        .find(|n| has(n));
    meta.readme = ["README", "README.md"].into_iter().find(|n| has(n));
    meta.submodules = has(".gitmodules");
    Ok(())
}
