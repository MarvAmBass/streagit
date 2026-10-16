//! History walk in `git log` order (SPEC §4.4, §6.2).

use gix::ObjectId;

/// Walk from `start` (inclusive), newest commit time first, returning up to `n` ids.
pub fn walk(
    repo: &gix::Repository,
    start: ObjectId,
    n: usize,
    mut check: impl FnMut() -> Result<(), crate::render::out::RenderError>,
) -> Result<Vec<ObjectId>, crate::render::out::RenderError> {
    let mut out = Vec::with_capacity(n.min(1024));
    if n == 0 {
        return Ok(out);
    }
    let sorting = gix::revision::walk::Sorting::ByCommitTime(
        gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
    );
    let iter = repo
        .rev_walk([start])
        .sorting(sorting)
        .all()
        .map_err(anyhow::Error::from)?;
    for (i, info) in iter.enumerate() {
        if i % 64 == 0 {
            check()?;
        }
        let info = info.map_err(anyhow::Error::from)?;
        out.push(info.id);
        if out.len() >= n {
            break;
        }
    }
    Ok(out)
}
