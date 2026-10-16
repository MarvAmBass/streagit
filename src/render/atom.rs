//! atom.xml and tags.xml (SPEC §4.12).

use gix::ObjectId;

use crate::git::commit::CommitInfo;
use crate::git::refs::Refs;
use crate::render::commit::write_commit_head;
use crate::render::header::Ctx;
use crate::render::out::{Out, R};
use crate::render::time::time_z;
use crate::w;

fn write_entry(out: &mut dyn Out, ctx: &Ctx, info: &CommitInfo, tag: Option<&[u8]>) -> R {
    w!(out, "<entry>\n<id>{}</id>\n", info.id)?;
    if let Some(a) = &info.author {
        w!(out, "<published>{}</published>\n", time_z(a.time))?;
    }
    if let Some(c) = &info.committer {
        w!(out, "<updated>{}</updated>\n", time_z(c.time))?;
    }
    out.write(b"<title>")?;
    if let Some(t) = tag {
        out.write(b"[")?;
        out.esc(t)?;
        out.write(b"] ")?;
    }
    out.esc(&info.summary)?;
    out.write(b"</title>\n")?;
    w!(
        out,
        "<link rel=\"alternate\" type=\"text/html\" href=\"{}commit/{}.html\" />\n",
        ctx.base_url,
        info.id
    )?;
    if let Some(a) = &info.author {
        out.write(b"<author>\n<name>")?;
        out.esc(&a.name)?;
        out.write(b"</name>\n<email>")?;
        out.esc(&a.email)?;
        out.write(b"</email>\n</author>\n")?;
    }
    out.write(b"<content>")?;
    write_commit_head(out, info, false)?;
    out.write(b"\n</content>\n</entry>\n")
}

fn write_feed_head(out: &mut dyn Out, ctx: &Ctx) -> R {
    out.write(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n<title>")?;
    out.esc(ctx.meta.name.as_bytes())?;
    out.write(b", branch HEAD</title>\n<subtitle>")?;
    out.esc(&ctx.meta.description)?;
    out.write(b"</subtitle>\n")?;
    out.flush()
}

pub fn render_atom(
    repo: &gix::Repository,
    ctx: &Ctx,
    tip: Option<ObjectId>,
    out: &mut dyn Out,
) -> R {
    write_feed_head(out, ctx)?;
    let ids = match tip {
        Some(t) => crate::git::log::walk(repo, t, ctx.feed_entries, || ctx.check())?,
        None => Vec::new(),
    };
    for id in ids {
        ctx.check()?;
        let commit = repo.find_commit(id).map_err(anyhow::Error::from)?;
        let info = CommitInfo::from_commit(&commit)?;
        write_entry(out, ctx, &info, None)?;
    }
    out.write(b"</feed>\n")
}

pub fn render_tags(ctx: &Ctx, refs: &Refs, out: &mut dyn Out) -> R {
    write_feed_head(out, ctx)?;
    for r in refs.tags.iter().take(ctx.feed_entries) {
        ctx.check()?;
        let info = CommitInfo {
            id: r.id,
            tree: r.id,
            parent: r.parent,
            author: r.author.clone(),
            committer: r.committer.clone(),
            summary: r.summary.clone(),
            message: r.message.clone(),
        };
        write_entry(out, ctx, &info, Some(&r.name))?;
    }
    out.write(b"</feed>\n")
}
