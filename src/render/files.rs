//! files.html (SPEC §4.6, §6.4).

use gix::object::tree::EntryKind;
use gix::ObjectId;

use crate::cache::stats::StatsCache;
use crate::git::blob::{is_binary, line_count};
use crate::git::tree::{list_files, mode_string};
use crate::render::header::{write_footer, write_header, Ctx};
use crate::render::out::{Out, R};
use crate::w;

pub fn render_files(
    repo: &gix::Repository,
    ctx: &Ctx,
    stats: &StatsCache,
    tree: Option<ObjectId>,
    out: &mut dyn Out,
) -> R {
    write_header(out, ctx, b"Files")?;
    out.write(b"<table id=\"files\"><thead>\n<tr><td><b>Mode</b></td><td><b>Name</b></td><td class=\"num\" align=\"right\"><b>Size</b></td></tr>\n</thead><tbody>\n")?;
    let (entries, truncated) = match tree {
        Some(t) => list_files(repo, t, ctx.max_files, || ctx.check())?,
        None => (Vec::new(), false),
    };
    for (i, e) in entries.iter().enumerate() {
        if i % 256 == 0 {
            ctx.check()?;
        }
        w!(out, "<tr><td>{}</td><td>", mode_string(e.mode, e.kind))?;
        if e.kind == EntryKind::Commit {
            out.write(b"<a href=\"file/.gitmodules.html\">")?;
            out.esc(&e.path)?;
            w!(
                out,
                "</a> @ {}</td><td class=\"num\" align=\"right\"></td></tr>\n",
                &e.id.to_string()[..7]
            )?;
            continue;
        }
        out.write(b"<a href=\"file/")?;
        out.pct(&e.path)?;
        out.write(b".html\">")?;
        out.esc(&e.path)?;
        out.write(b"</a></td><td class=\"num\" align=\"right\">")?;
        let header = repo.find_header(e.id).map_err(anyhow::Error::from)?;
        let size = header.size();
        let mut written = false;
        if ctx.files_line_counts {
            let lines = match stats.get_lines(&e.id) {
                Some(n) => n,
                None => {
                    let data = &repo.find_object(e.id).map_err(anyhow::Error::from)?.data;
                    let n = if is_binary(data) {
                        0
                    } else {
                        line_count(data) as u32
                    };
                    stats.put_lines(e.id, n);
                    n
                }
            };
            if lines > 0 {
                w!(out, "{lines}L")?;
                written = true;
            }
        }
        if !written {
            w!(out, "{size}B")?;
        }
        out.write(b"</td></tr>\n")?;
        if i % 200 == 199 {
            out.flush()?;
        }
    }
    if truncated {
        out.write(b"<tr><td></td><td colspan=\"2\">&hellip; more files not shown</td></tr>\n")?;
    }
    out.write(b"</tbody></table>")?;
    write_footer(out)
}
