//! refs.html (SPEC §4.8).

use crate::git::refs::{RefInfo, Refs};
use crate::render::header::{write_footer, write_header, Ctx};
use crate::render::out::{Out, R};
use crate::render::time::time_short;
use crate::w;

fn write_group(out: &mut dyn Out, ctx: &Ctx, title: &str, id: &str, rows: &[RefInfo]) -> R {
    if rows.is_empty() {
        return Ok(());
    }
    w!(out, "<h2>{title}</h2><table id=\"{id}\"><thead>\n<tr><td><b>Name</b></td><td><b>Last commit date</b></td><td><b>Author</b></td>\n</tr>\n</thead><tbody>\n")?;
    for (i, r) in rows.iter().enumerate() {
        if i % 256 == 0 {
            ctx.check()?;
        }
        out.write(b"<tr><td>")?;
        if ctx.link_refs {
            out.write(b"<a href=\"log.html?ref=")?;
            out.pct(&r.name)?;
            out.write(b"\">")?;
            out.esc(&r.name)?;
            out.write(b"</a>")?;
        } else {
            out.esc(&r.name)?;
        }
        out.write(b"</td><td>")?;
        if let Some(a) = &r.author {
            out.write(time_short(a.time).as_bytes())?;
        }
        out.write(b"</td><td>")?;
        if let Some(a) = &r.author {
            out.esc(&a.name)?;
        }
        out.write(b"</td></tr>\n")?;
        if i % 200 == 199 {
            out.flush()?;
        }
    }
    out.write(b"</tbody></table><br/>\n")
}

pub fn render_refs(ctx: &Ctx, refs: &Refs, out: &mut dyn Out) -> R {
    write_header(out, ctx, b"Refs")?;
    write_group(out, ctx, "Branches", "branches", &refs.branches)?;
    write_group(out, ctx, "Tags", "tags", &refs.tags)?;
    write_footer(out)
}
