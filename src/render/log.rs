//! log.html (SPEC §4.4, §6.2).

use gix::ObjectId;

use crate::cache::stats::StatsCache;
use crate::git::commit::CommitInfo;
use crate::git::diff::{diff_commit, Mode, Stats};
use crate::render::header::{write_footer, write_header, Ctx};
use crate::render::out::{Out, R};
use crate::render::time::time_short;
use crate::w;

/// Stats for one commit: from the cache or a counts-only diff.
pub fn stats_for(
    repo: &gix::Repository,
    ctx: &Ctx,
    stats: &StatsCache,
    info: &CommitInfo,
) -> Result<Stats, crate::render::out::RenderError> {
    if let Some(s) = stats.get_stats(&info.id) {
        return Ok(s);
    }
    let d = diff_commit(repo, info, Mode::CountsOnly, ctx.limits, || ctx.check())?;
    stats.put_stats(info.id, d.stats);
    Ok(d.stats)
}

pub fn write_log_row(out: &mut dyn Out, r: &str, info: &CommitInfo, s: Stats) -> R {
    out.write(b"<tr><td>")?;
    if let Some(a) = &info.author {
        out.write(time_short(a.time).as_bytes())?;
    }
    out.write(b"</td><td>")?;
    w!(out, "<a href=\"{r}commit/{}.html\">", info.id)?;
    out.esc(&info.summary)?;
    out.write(b"</a></td><td>")?;
    if let Some(a) = &info.author {
        out.esc(&a.name)?;
    }
    w!(
        out,
        "</td><td class=\"num\" align=\"right\">{}</td><td class=\"num\" align=\"right\">+{}</td><td class=\"num\" align=\"right\">-{}</td></tr>\n",
        s.files,
        s.adds,
        s.dels
    )
}

/// Render the log page starting at `start` (the tip, or `?from=`), `page_size` rows.
/// `query_suffix` is appended to pagination links (`&ref=name`).
pub fn render_log(
    repo: &gix::Repository,
    ctx: &Ctx,
    stats: &StatsCache,
    start: Option<ObjectId>,
    is_from: bool,
    query_suffix: &str,
    out: &mut dyn Out,
) -> R {
    write_header(out, ctx, b"Log")?;
    out.write(b"<table id=\"log\"><thead>\n<tr><td><b>Date</b></td><td><b>Commit message</b></td><td><b>Author</b></td><td class=\"num\" align=\"right\"><b>Files</b></td><td class=\"num\" align=\"right\"><b>+</b></td><td class=\"num\" align=\"right\"><b>-</b></td></tr>\n</thead><tbody>\n")?;
    let n = ctx.page_size;
    let ids = match start {
        Some(s) => crate::git::log::walk(repo, s, n + 1, || ctx.check())?,
        None => Vec::new(),
    };
    if is_from {
        let sep = if query_suffix.is_empty() { "" } else { "?" };
        w!(out, "<tr><td></td><td colspan=\"5\"><a href=\"log.html{sep}{}\">&uarr; newest</a></td></tr>\n", query_suffix.trim_start_matches('&'))?;
    }
    if start.is_none() {
        out.write(b"<tr><td></td><td colspan=\"5\">empty repository</td></tr>\n")?;
    }
    for (i, id) in ids.iter().take(n).enumerate() {
        ctx.check()?;
        let commit = repo.find_commit(*id).map_err(anyhow::Error::from)?;
        let info = CommitInfo::from_commit(&commit)?;
        let s = stats_for(repo, ctx, stats, &info)?;
        write_log_row(out, "", &info, s)?;
        if i % 20 == 19 {
            out.flush()?;
        }
    }
    if ids.len() > n {
        w!(out, "<tr><td></td><td colspan=\"5\"><a href=\"log.html?from={}{}\">Next {} commits &rarr;</a></td></tr>\n", ids[n], query_suffix, n)?;
    }
    out.write(b"</tbody></table>")?;
    write_footer(out)
}
