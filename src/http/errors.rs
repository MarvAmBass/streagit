//! Error pages (SPEC §5.5).

use axum::body::Body;
use axum::http::{header, HeaderValue, Response, StatusCode};

use crate::git::repo::Meta;
use crate::render::header::{write_footer, write_header, Ctx};
use crate::render::out::{Out, VecOut};

/// A minimal page for errors where no repository (and so no stylesheet) is known.
pub fn plain_page(status: StatusCode, what: &str) -> Response<Body> {
    let mut v = VecOut::default();
    let code = status.as_u16();
    let reason = status.canonical_reason().unwrap_or("");
    let _ = v.write(b"<!DOCTYPE html>\n<html>\n<head>\n<meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n<title>");
    let _ = v.fmt(format_args!("{code} {reason}"));
    let _ = v.write(
        b"</title>\n<style>body{font-family:monospace;margin:1em}</style>\n</head>\n<body>\n<h1>",
    );
    let _ = v.fmt(format_args!("{code} {reason}"));
    let _ = v.write(b"</h1>\n<p>");
    let _ = v.esc(what.as_bytes());
    let _ = v.write(b"</p>\n</body>\n</html>\n");
    html_response(status, v.0.into())
}

/// An error page wrapped in the repository's stagit header/footer.
pub fn repo_page(status: StatusCode, what: &str, ctx: &Ctx) -> Response<Body> {
    let mut v = VecOut::default();
    let title = format!(
        "{} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    );
    let _ = write_header(&mut v, ctx, title.as_bytes());
    let _ = v.write(b"<p>");
    let _ = v.esc(title.as_bytes());
    let _ = v.write(b": ");
    let _ = v.esc(what.as_bytes());
    let _ = v.write(b"</p>\n");
    let _ = write_footer(&mut v);
    html_response(status, v.0.into())
}

pub fn html_response(status: StatusCode, body: Body) -> Response<Body> {
    let mut r = Response::new(body);
    *r.status_mut() = status;
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    r
}

pub fn overloaded() -> Response<Body> {
    let mut r = plain_page(
        StatusCode::SERVICE_UNAVAILABLE,
        "The renderer is busy, try again shortly.",
    );
    r.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
    r
}

/// Build a `Ctx` for an error page from repository metadata.
pub fn error_ctx(meta: Meta, relpath: String) -> Ctx {
    Ctx {
        meta,
        relpath,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
        page_size: 0,
        feed_entries: 0,
        max_files: 0,
        files_line_counts: false,
        max_blob_bytes: 0,
        limits: crate::git::diff::Limits {
            max_diff_blob_bytes: 0,
            max_commit_diff_bytes: 0,
        },
        link_refs: false,
        base_url: String::new(),
    }
}
