//! Branches and tags with their peeled commit (SPEC §4.8).

use gix::ObjectId;

use super::commit::Sig;

#[derive(Debug, Clone)]
pub struct RefInfo {
    /// Shorthand name (`main`, `v1.0`).
    pub name: Vec<u8>,
    pub id: ObjectId,
    pub author: Option<Sig>,
    pub committer: Option<Sig>,
    pub summary: Vec<u8>,
    pub message: Vec<u8>,
    pub parent: Option<ObjectId>,
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

/// Collect branches and tags, peeled to commits, sorted like stagit:
/// author time descending, then name ascending.
pub fn list_refs(repo: &gix::Repository) -> anyhow::Result<Refs> {
    let platform = repo.references()?;
    let mut out = Refs::default();
    for (iter, is_tag) in [
        (platform.local_branches()?, false),
        (platform.tags()?, true),
    ] {
        let iter = iter.peeled()?;
        for r in iter {
            let r = r?;
            let Some(id) = r.target().try_id().map(|i| i.to_owned()) else {
                continue;
            };
            // stagit stops at the first tag that peels to a non-commit; we skip it
            let Ok(obj) = repo.find_object(id) else {
                continue;
            };
            if obj.kind != gix::object::Kind::Commit {
                continue;
            }
            let commit = obj.into_commit();
            let Ok(info) = super::commit::CommitInfo::from_commit(&commit) else {
                continue;
            };
            let name = r.name().shorten().to_vec();
            let ri = RefInfo {
                name,
                id,
                author: info.author,
                committer: info.committer,
                summary: info.summary,
                message: info.message,
                parent: info.parent,
            };
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
