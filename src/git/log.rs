//! History walk in `git log` order (SPEC §4.4, §6.2).

use gix::ObjectId;

use crate::render::out::RenderError;

/// Commit ids reachable from `start`, newest commit time first (`git log` order).
fn iter(
    repo: &gix::Repository,
    start: ObjectId,
) -> Result<impl Iterator<Item = anyhow::Result<ObjectId>> + '_, RenderError> {
    let sorting = gix::revision::walk::Sorting::ByCommitTime(
        gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
    );
    let it = repo
        .rev_walk([start])
        .sorting(sorting)
        .all()
        .map_err(anyhow::Error::from)?;
    Ok(it.map(|r| r.map(|info| info.id).map_err(anyhow::Error::from)))
}

/// Walk from `start` (inclusive), returning up to `n` ids.
pub fn walk(
    repo: &gix::Repository,
    start: ObjectId,
    n: usize,
    mut check: impl FnMut() -> Result<(), RenderError>,
) -> Result<Vec<ObjectId>, RenderError> {
    let mut out = Vec::with_capacity(n.min(1024));
    if n == 0 {
        return Ok(out);
    }
    for (i, id) in iter(repo, start)?.enumerate() {
        if i % 64 == 0 {
            check()?;
        }
        out.push(id?);
        if out.len() >= n {
            break;
        }
    }
    Ok(out)
}

/// The page of up to `n` commits of the walk from `tip` that starts at `from`.
///
/// The walk is restarted from `tip` and skipped forward to `from` rather than
/// started at `from`: a date-ordered walk from a single commit loses the other
/// branches that were still pending in the queue when the previous page ended
/// (their commits are not reachable from `from`), which would drop commits
/// between pages. Skipping only decodes commit headers, no diffs. If `from` is
/// not reachable from `tip` the walk simply starts at `from`.
pub fn walk_from(
    repo: &gix::Repository,
    tip: ObjectId,
    from: ObjectId,
    n: usize,
    mut check: impl FnMut() -> Result<(), RenderError>,
) -> Result<Vec<ObjectId>, RenderError> {
    if tip == from {
        return walk(repo, tip, n, check);
    }
    let mut out = Vec::with_capacity(n.min(1024));
    let mut found = false;
    for (i, id) in iter(repo, tip)?.enumerate() {
        if i % 64 == 0 {
            check()?;
        }
        let id = id?;
        if !found {
            if id != from {
                continue;
            }
            found = true;
        }
        out.push(id);
        if out.len() >= n {
            break;
        }
    }
    if !found {
        return walk(repo, from, n, check);
    }
    Ok(out)
}
