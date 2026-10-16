//! stagit's `writeheader` / `writefooter` (SPEC §4.3) and the render context.

use std::time::Instant;

use crate::git::repo::Meta;
use crate::render::out::{Out, RenderError, R};
use crate::w;

/// Everything a page render needs besides the repository.
#[derive(Clone)]
pub struct Ctx {
    pub meta: Meta,
    /// `""` for top-level pages, `"../"` for `commit/`, one `../` per directory level for `file/`.
    pub relpath: String,
    pub deadline: Instant,
    pub page_size: usize,
    pub feed_entries: usize,
    pub max_files: usize,
    pub files_line_counts: bool,
    pub max_blob_bytes: u64,
    pub limits: crate::git::diff::Limits,
    pub link_refs: bool,
    /// Absolute base for atom links (`--public-url` + repo prefix + `/`), empty for relative.
    pub base_url: String,
}

impl Ctx {
    /// Deadline check, called between rows/deltas/lines.
    pub fn check(&self) -> R {
        if Instant::now() >= self.deadline {
            Err(RenderError::Timeout)
        } else {
            Ok(())
        }
    }
}

pub fn write_header(out: &mut dyn Out, ctx: &Ctx, title: &[u8]) -> R {
    let m = &ctx.meta;
    let r = ctx.relpath.as_str();
    out.write(b"<!DOCTYPE html>\n<html>\n<head>\n<meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n<title>")?;
    out.esc(title)?;
    if !title.is_empty() && !m.name.is_empty() {
        out.write(b" - ")?;
    }
    out.esc(m.name.as_bytes())?;
    if !m.description.is_empty() {
        out.write(b" - ")?;
        out.esc(&m.description)?;
    }
    w!(
        out,
        "</title>\n<link rel=\"icon\" type=\"image/png\" href=\"{r}favicon.png\" />\n"
    )?;
    out.write(b"<link rel=\"alternate\" type=\"application/atom+xml\" title=\"")?;
    out.esc(m.name.as_bytes())?;
    w!(out, " Atom Feed\" href=\"{r}atom.xml\" />\n")?;
    out.write(b"<link rel=\"alternate\" type=\"application/atom+xml\" title=\"")?;
    out.esc(m.name.as_bytes())?;
    w!(out, " Atom Feed (tags)\" href=\"{r}tags.xml\" />\n")?;
    w!(
        out,
        "<link rel=\"stylesheet\" type=\"text/css\" href=\"{r}style.css\" />\n</head>\n<body>\n"
    )?;
    w!(out, "<table><tr><td><a href=\"../{r}\"><img src=\"{r}logo.png\" alt=\"\" width=\"32\" height=\"32\" /></a></td><td><h1>")?;
    out.esc(m.name.as_bytes())?;
    out.write(b"</h1><span class=\"desc\">")?;
    out.esc(&m.description)?;
    out.write(b"</span></td></tr>")?;
    if !m.url.is_empty() {
        out.write(b"<tr class=\"url\"><td></td><td>git clone <a href=\"")?;
        out.esc(&m.url)?;
        out.write(b"\">")?;
        out.esc(&m.url)?;
        out.write(b"</a></td></tr>")?;
    }
    out.write(b"<tr><td></td><td>\n")?;
    w!(out, "<a href=\"{r}log.html\">Log</a> | <a href=\"{r}files.html\">Files</a> | <a href=\"{r}refs.html\">Refs</a>")?;
    if m.submodules {
        w!(
            out,
            " | <a href=\"{r}file/.gitmodules.html\">Submodules</a>"
        )?;
    }
    if let Some(readme) = m.readme {
        w!(out, " | <a href=\"{r}file/{readme}.html\">README</a>")?;
    }
    if let Some(license) = m.license {
        w!(out, " | <a href=\"{r}file/{license}.html\">LICENSE</a>")?;
    }
    out.write(b"</td></tr></table>\n<hr/>\n<div id=\"content\">\n")?;
    out.flush()
}

pub fn write_footer(out: &mut dyn Out) -> R {
    out.write(b"</div>\n</body>\n</html>\n")
}
