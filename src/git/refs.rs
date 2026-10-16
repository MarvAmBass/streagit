//! Branches and tags with their peeled commit (SPEC §4.8).

use std::collections::HashMap;

use gix::ObjectId;

use super::commit::{CommitInfo, Sig};
use crate::render::out::RenderError;

/// One branch or tag row. Only what refs.html needs is kept: a repository with
/// tens of thousands of tags must not retain a commit message per ref (tags.xml
/// loads the commits of its top entries on demand, `commit_info`).
#[derive(Debug, Clone)]
pub struct RefInfo {
    /// Shorthand name (`main`, `v1.0`).
    pub name: Vec<u8>,
    /// The peeled commit.
    pub id: ObjectId,
    pub author: Option<Sig>,
}

impl RefInfo {
    /// Full commit data for feed entries.
    pub fn commit_info(&self, repo: &gix::Repository) -> anyhow::Result<CommitInfo> {
        let commit = repo.find_commit(self.id)?;
        CommitInfo::from_commit(&commit)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Refs {
    pub branches: Vec<RefInfo>,
    pub tags: Vec<RefInfo>,
}

/// Hash of all (name, target) pairs under `refs/heads` and `refs/tags`, used as the
/// cache key for refs.html / tags.xml. Cheap: no object is read.
pub fn refs_hash(repo: &gix::Repository) -> anyhow::Result<u64> {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    let platform = repo.references()?;
    for iter in [platform.local_branches()?, platform.tags()?] {
        for r in iter {
            let r = r?;
            h.update(r.name().as_bstr());
            h.update(b"=");
            match r.target().try_id() {
                Some(id) => h.update(id.as_bytes()),
                None => h.update(b"sym"),
            }
            h.update(b"\n");
        }
    }
    Ok(h.digest())
}

/// Peel `id` to a commit and return it with its author; `None` if it peels to
/// something else (tree, blob) or cannot be read. Only object *headers* are inspected
/// until a tag object has to be followed: gix's `peeled()` iterator would load
/// the whole target object for every ref, which is prohibitive with tens of
/// thousands of lightweight tags (or a tag on a huge blob).
fn peel_to_commit(repo: &gix::Repository, mut id: ObjectId) -> Option<(ObjectId, Option<Sig>)> {
    for _ in 0..16 {
        match repo.try_find_header(id) {
            Ok(Some(h)) if h.kind() == gix::object::Kind::Commit => {
                let commit = repo.find_commit(id).ok()?;
                return Some((id, commit.author().ok().map(|a| Sig::from_ref(&a))));
            }
            Ok(Some(h)) if h.kind() == gix::object::Kind::Tag => {
                let tag = repo.find_tag(id).ok()?;
                id = tag.target_id().ok()?.detach();
            }
            _ => return None,
        }
    }
    None
}

/// Collect branches and tags, peeled to commits, sorted like stagit:
/// author time descending, then name ascending. `check` (the render deadline)
/// is called every few hundred refs.
pub fn list_refs(
    repo: &gix::Repository,
    mut check: impl FnMut() -> Result<(), RenderError>,
) -> Result<Refs, RenderError> {
    let platform = repo.references().map_err(anyhow::Error::from)?;
    let mut out = Refs::default();
    // many refs usually point at the same few commits (release tags, branches);
    // keyed by the unpeeled target so that annotated tags are peeled once too
    let mut peeled: HashMap<ObjectId, Option<(ObjectId, Option<Sig>)>> = HashMap::new();
    for (iter, is_tag) in [
        (
            platform.local_branches().map_err(anyhow::Error::from)?,
            false,
        ),
        (platform.tags().map_err(anyhow::Error::from)?, true),
    ] {
        for (i, r) in iter.enumerate() {
            if i % 256 == 0 {
                check()?;
            }
            let r = r.map_err(anyhow::Error::from)?;
            let name = r.name().shorten().to_vec();
            let raw = r.inner;
            // `packed-refs` carries the peeled commit of annotated tags (`^` lines)
            let target = match raw
                .peeled
                .or_else(|| raw.target.try_id().map(ToOwned::to_owned))
            {
                Some(id) => id,
                None => continue, // symbolic ref
            };
            // stagit stops at the first tag that peels to a non-commit; we skip it
            let Some((id, author)) = peeled
                .entry(target)
                .or_insert_with(|| peel_to_commit(repo, target))
                .clone()
            else {
                continue;
            };
            let ri = RefInfo { name, id, author };
            if is_tag {
                out.tags.push(ri);
            } else {
                out.branches.push(ri);
            }
        }
    }
    let key = |r: &RefInfo| {
        (
            std::cmp::Reverse(r.author.as_ref().map(|a| a.time).unwrap_or(0)),
            r.name.clone(),
        )
    };
    out.branches.sort_by_key(key);
    out.tags.sort_by_key(key);
    Ok(out)
}
