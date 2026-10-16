//! commit/<oid>.html (SPEC §4.5, §6.3, §7.2).

use gix::ObjectId;

use crate::cache::stats::StatsCache;
use crate::git::commit::CommitInfo;
use crate::git::diff::{diff_commit, FileDiff, LineKind, Mode, MAX_DELTAS};
use crate::render::header::{write_footer, write_header, Ctx};
use crate::render::out::{Out, R};
use crate::render::time::time_long;
use crate::w;

/// The `commit`/`parent`/`Author:`/`Date:`/message block shared with the atom feed.
pub fn write_commit_head(out: &mut dyn Out, info: &CommitInfo, html: bool) -> R {
    if html {
        w!(
            out,
            "<b>commit</b> <a href=\"../commit/{0}.html\">{0}</a>\n",
            info.id
        )?;
        if let Some(p) = &info.parent {
            w!(
                out,
                "<b>parent</b> <a href=\"../commit/{0}.html\">{0}</a>\n",
                p
            )?;
        }
        if let Some(a) = &info.author {
            out.write(b"<b>Author:</b> ")?;
            out.esc(&a.name)?;
            out.write(b" &lt;<a href=\"mailto:")?;
            out.esc(&a.email)?;
            out.write(b"\">")?;
            out.esc(&a.email)?;
            w!(
                out,
                "</a>&gt;\n<b>Date:</b>   {}\n",
                time_long(a.time, a.offset_min)
            )?;
        }
        out.write(b"\n")?;
        out.esc(&info.message)?;
        out.write(b"\n")?;
    } else {
        w!(out, "commit {}\n", info.id)?;
        if let Some(p) = &info.parent {
            w!(out, "parent {}\n", p)?;
        }
        if let Some(a) = &info.author {
            out.write(b"Author: ")?;
            out.esc(&a.name)?;
            out.write(b" &lt;")?;
            out.esc(&a.email)?;
            w!(out, "&gt;\nDate:   {}\n", time_long(a.time, a.offset_min))?;
        }
        // stagit prints the message block whenever msg is non-NULL, i.e. always
        out.write(b"\n")?;
        out.esc(&info.message)?;
    }
    Ok(())
}

pub fn render_commit(
    repo: &gix::Repository,
    ctx: &Ctx,
    stats: &StatsCache,
    id: ObjectId,
    out: &mut dyn Out,
) -> R {
    let commit = repo.find_commit(id).map_err(anyhow::Error::from)?;
    let info = CommitInfo::from_commit(&commit)?;
    write_header(out, ctx, &info.summary)?;
    out.write(b"<pre>")?;
    write_commit_head(out, &info, true)?;

    let d = diff_commit(repo, &info, Mode::Full, ctx.limits, || ctx.check())?;
    stats.put_stats(info.id, d.stats);

    if d.too_large || d.deltas.len() > MAX_DELTAS {
        out.write(b"Diff is too large, output suppressed.\n")?;
        out.write(b"</pre>\n")?;
        return write_footer(out);
    }

    // diffstat
    out.write(b"<b>Diffstat:</b>\n<table>")?;
    for (i, (delta, c)) in d.deltas.iter().zip(&d.counts).enumerate() {
        let letter = delta.status.letter();
        w!(
            out,
            "<tr><td class=\"{letter}\">{letter}</td><td><a href=\"#h{i}\">"
        )?;
        out.esc(&delta.old_path)?;
        if delta.old_path != delta.new_path {
            out.write(b" -&gt; ")?;
            out.esc(&delta.new_path)?;
        }
        let changed = c.adds + c.dels;
        w!(
            out,
            "</a></td><td> | </td><td class=\"num\">{changed}</td><td><span class=\"i\">"
        )?;
        let (mut add, mut del) = (c.adds as usize, c.dels as usize);
        if changed > 78 {
            // stagit: ((float)total / changed * add) + 1, truncated
            if add > 0 {
                add = (78f32 / changed as f32 * add as f32) as usize + 1;
            }
            if del > 0 {
                del = (78f32 / changed as f32 * del as f32) as usize + 1;
            }
        }
        out.write(&vec![b'+'; add])?;
        out.write(b"</span><span class=\"d\">")?;
        out.write(&vec![b'-'; del])?;
        out.write(b"</span></td></tr>\n")?;
    }
    let n = d.deltas.len();
    w!(
        out,
        "</table></pre><pre>{} file{} changed, {} insertion{}(+), {} deletion{}(-)\n",
        n,
        if n == 1 { "" } else { "s" },
        d.stats.adds,
        if d.stats.adds == 1 { "" } else { "s" },
        d.stats.dels,
        if d.stats.dels == 1 { "" } else { "s" }
    )?;
    out.write(b"<hr/>")?;
    out.flush()?;

    // diffs
    for (i, (delta, file)) in d.deltas.iter().zip(&d.files).enumerate() {
        ctx.check()?;
        w!(out, "<b>diff --git a/<a id=\"h{i}\" href=\"../file/")?;
        out.pct(&delta.old_path)?;
        out.write(b".html\">")?;
        out.esc(&delta.old_path)?;
        out.write(b"</a> b/<a href=\"../file/")?;
        out.pct(&delta.new_path)?;
        out.write(b".html\">")?;
        out.esc(&delta.new_path)?;
        out.write(b"</a></b>\n")?;
        match file {
            FileDiff::Binary => out.write(b"Binary files differ.\n")?,
            FileDiff::Skipped(reason) => {
                out.write(reason.as_bytes())?;
                out.write(b"\n")?;
            }
            FileDiff::Text { old, new, hunks } => {
                for (j, h) in hunks.iter().enumerate() {
                    w!(out, "<a href=\"#h{i}-{j}\" id=\"h{i}-{j}\" class=\"h\">")?;
                    out.esc(&h.header)?;
                    out.write(b"</a>")?;
                    for (k, l) in h.lines.iter().enumerate() {
                        if l.marker {
                            match l.kind {
                                LineKind::Context => out.write(b" \\ No newline at end of file\n")?,
                                LineKind::Add => w!(out, "<a href=\"#h{i}-{j}-{k}\" id=\"h{i}-{j}-{k}\" class=\"i\">+\\ No newline at end of file\n</a>")?,
                                LineKind::Del => w!(out, "<a href=\"#h{i}-{j}-{k}\" id=\"h{i}-{j}-{k}\" class=\"d\">-\\ No newline at end of file\n</a>")?,
                            }
                            continue;
                        }
                        match l.kind {
                            LineKind::Context => {
                                out.write(b" ")?;
                                out.escl(&old[l.start..l.end])?;
                                out.write(b"\n")?;
                            }
                            LineKind::Del => {
                                w!(
                                    out,
                                    "<a href=\"#h{i}-{j}-{k}\" id=\"h{i}-{j}-{k}\" class=\"d\">-"
                                )?;
                                out.escl(&old[l.start..l.end])?;
                                out.write(b"\n</a>")?;
                            }
                            LineKind::Add => {
                                w!(
                                    out,
                                    "<a href=\"#h{i}-{j}-{k}\" id=\"h{i}-{j}-{k}\" class=\"i\">+"
                                )?;
                                out.escl(&new[l.start..l.end])?;
                                out.write(b"\n</a>")?;
                            }
                        }
                    }
                    if j % 8 == 7 {
                        out.flush()?;
                    }
                }
            }
        }
    }
    out.write(b"</pre>\n")?;
    write_footer(out)
}
