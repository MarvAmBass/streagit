//! file/<path>.html (SPEC §4.7, §6.5).

use gix::ObjectId;

use crate::git::blob::{is_binary, lines_with_terminator};
use crate::render::header::{write_footer, write_header, Ctx};
use crate::render::out::{Out, R};
use crate::w;

/// `path` is the full path inside the tree; `raw_href` links to the raw blob.
pub fn render_blob(
    repo: &gix::Repository,
    ctx: &Ctx,
    id: ObjectId,
    path: &[u8],
    out: &mut dyn Out,
) -> R {
    let filename = path.rsplit(|&c| c == b'/').next().unwrap_or(path);
    let obj = repo.find_object(id).map_err(anyhow::Error::from)?;
    let data: &[u8] = &obj.data;
    write_header(out, ctx, filename)?;
    out.write(b"<p> ")?;
    out.esc(filename)?;
    w!(out, " ({}B)</p><hr/>", data.len())?;
    if is_binary(data) {
        out.write(b"<p>Binary file.</p>\n")?;
        return write_footer(out);
    }
    let max = ctx.max_blob_bytes as usize;
    let (shown, truncated) = if data.len() > max {
        // cut at the last newline within the limit
        let cut = data[..max]
            .iter()
            .rposition(|&c| c == b'\n')
            .map(|i| i + 1)
            .unwrap_or(max);
        (&data[..cut], true)
    } else {
        (data, false)
    };
    out.write(b"<pre id=\"blob\">\n")?;
    let mut n = 0usize;
    for line in lines_with_terminator(shown) {
        n += 1;
        if n % 512 == 0 {
            ctx.check()?;
            out.flush()?;
        }
        w!(
            out,
            "<a href=\"#l{n}\" class=\"line\" id=\"l{n}\">{n:>7}</a> "
        )?;
        out.escl(line)?;
        if line.ends_with(b"\n") {
            out.write(b"\n")?;
        }
    }
    out.write(b"</pre>\n")?;
    if truncated {
        w!(
            out,
            "<p>File truncated, {} bytes not shown. <a href=\"{}raw/",
            data.len() - shown.len(),
            ctx.relpath
        )?;
        out.pct(path)?;
        out.write(b"\">Raw file</a></p>\n")?;
    }
    write_footer(out)
}
