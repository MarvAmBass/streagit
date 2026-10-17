//! Request dispatch: resolution, ETag/304, caches, render permits, streaming (SPEC §5, §7, §8, §9).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, Response, StatusCode};
use bytes::Bytes;
use gix::ObjectId;
use tokio::sync::{mpsc, Semaphore};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use super::assets::Assets;
use super::errors::{error_ctx, overloaded, plain_page, repo_page};
use super::resolve::{resolve, split_path, PathError, Resolved};
use super::stream::{ChunkOut, CHANNEL_DEPTH};
use crate::cache::inflight::{follow, InFlight, Ticket};
use crate::cache::render::RenderCache;
use crate::cache::stats::StatsCache;
use crate::config::{Config, Robots};
use crate::git::blob::is_binary;
use crate::git::repo::{detect_links, read_meta, resolve_head, resolve_shorthand, RepoCache};
use crate::render::header::{write_footer, Ctx};
use crate::render::out::{Out, RenderError};

#[derive(Default)]
pub struct Metrics {
    pub requests: AtomicU64,
    pub renders: AtomicU64,
    pub render_millis: AtomicU64,
    pub cache_hits: AtomicU64,
    pub not_modified: AtomicU64,
    pub queue_drops: AtomicU64,
    pub timeouts: AtomicU64,
    pub slow_clients: AtomicU64,
    pub errors: AtomicU64,
}

pub struct AppState {
    pub cfg: Config,
    pub prefix: String,
    pub root: PathBuf,
    pub canonical_root: PathBuf,
    pub repos: RepoCache,
    pub render_cache: RenderCache,
    pub stats: StatsCache,
    pub inflight: Arc<InFlight>,
    pub permits: Arc<Semaphore>,
    pub waiting: AtomicUsize,
    pub assets: Assets,
    pub tv: String,
    pub metrics: Metrics,
    pub excludes: crate::exclude::Excludes,
    /// `Some` with `--index true`.
    pub index: Option<crate::index::IndexCache>,
    /// `Some` when basic auth is configured.
    pub auth: Option<crate::auth::Auth>,
}

impl AppState {
    pub fn new(cfg: Config) -> anyhow::Result<AppState> {
        let root = cfg.root.clone();
        let canonical_root = std::fs::canonicalize(&root)
            .map_err(|e| anyhow::anyhow!("root {}: {e}", root.display()))?;
        let stats = match &cfg.cache_dir {
            Some(dir) => StatsCache::with_dir(cfg.stats_entries, dir)?,
            None => StatsCache::new(cfg.stats_entries),
        };
        let auth = crate::auth::Auth::load(
            cfg.auth_file.as_deref(),
            cfg.auth_user.as_deref(),
            cfg.auth_password.as_ref().map(|s| s.0.as_str()),
        )?;
        Ok(AppState {
            excludes: crate::exclude::Excludes::new(&cfg.exclude),
            index: cfg
                .index
                .then(|| crate::index::IndexCache::new(cfg.index_ttl)),
            auth,
            prefix: cfg.prefix(),
            tv: cfg.template_version(),
            repos: RepoCache::new(cfg.open_repos, cfg.object_cache_bytes as usize),
            render_cache: RenderCache::new(
                cfg.cache_bytes as usize,
                10_000,
                cfg.cache_max_entry as usize,
            ),
            stats,
            inflight: Arc::new(InFlight::default()),
            permits: Arc::new(Semaphore::new(cfg.render_threads())),
            waiting: AtomicUsize::new(0),
            assets: Assets::load(cfg.assets_dir.as_deref()),
            metrics: Metrics::default(),
            root,
            canonical_root,
            cfg,
        })
    }

    /// Whether a resolved repository is hidden by `--exclude` (by the requested
    /// path or by where it really is).
    fn excluded(&self, r: &crate::git::repo::RepoRef) -> bool {
        if self.excludes.is_empty() {
            return false;
        }
        let canon = r
            .git_dir
            .strip_prefix(&self.canonical_root)
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.excludes.is_excluded(&r.rel) || self.excludes.is_excluded(&canon)
    }

    /// The index's repository list (cached for `--index-ttl`).
    pub async fn index_entries(self: &Arc<Self>) -> Option<Arc<Vec<crate::index::Entry>>> {
        let cache = self.index.as_ref()?;
        let st = self.clone();
        Some(
            cache
                .get(move || {
                    crate::index::build(&crate::index::Discovery {
                        root: &st.canonical_root,
                        depth: st.cfg.index_depth,
                        excludes: &st.excludes,
                        allow_git_paths: st.cfg.allow_git_paths,
                        allow_bare_dirs: st.cfg.allow_bare_dirs(),
                    })
                })
                .await,
        )
    }

    fn limits(&self) -> crate::git::diff::Limits {
        crate::git::diff::Limits {
            max_diff_blob_bytes: self.cfg.max_diff_blob_bytes,
            max_commit_diff_bytes: self.cfg.max_commit_diff_bytes,
        }
    }
}

/// Which page was asked for (SPEC §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Page {
    Log {
        from: Option<String>,
        reference: Option<String>,
    },
    Commit(String),
    Files,
    Refs,
    Atom,
    Tags,
    Blob(Vec<Vec<u8>>),
    Raw(Vec<Vec<u8>>),
}

/// `from` / `ref` query values. `None` for a query that is too long or carries a
/// value that is not a plain, decodable path segment (the page is then 404).
fn parse_query(q: Option<&str>) -> Option<(Option<String>, Option<String>)> {
    let Some(q) = q else {
        return Some((None, None));
    };
    if q.len() > 1024 {
        return None;
    }
    let (mut from, mut reference) = (None, None);
    for kv in q.split('&') {
        if kv.is_empty() {
            continue;
        }
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        if !matches!(k, "from" | "ref") {
            continue;
        }
        let v = super::resolve::decode_segment(v)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())?;
        match k {
            "from" => from = Some(v),
            _ => reference = Some(v),
        }
    }
    Some((from, reference))
}

/// Map page path segments to a `Page`; `None` for unknown paths.
pub fn classify(segs: &[Vec<u8>], query: Option<&str>) -> Option<Page> {
    match segs {
        [p] if p == b"log.html" => {
            let (from, reference) = parse_query(query)?;
            Some(Page::Log { from, reference })
        }
        [p] if p == b"files.html" => Some(Page::Files),
        [p] if p == b"refs.html" => Some(Page::Refs),
        [p] if p == b"atom.xml" => Some(Page::Atom),
        [p] if p == b"tags.xml" => Some(Page::Tags),
        [c, oid] if c == b"commit" => {
            let s = std::str::from_utf8(oid).ok()?.strip_suffix(".html")?;
            is_oid(s).then(|| Page::Commit(s.to_string()))
        }
        [f, rest @ ..] if f == b"file" && !rest.is_empty() => {
            let mut path = rest.to_vec();
            let last = path.last_mut()?;
            let stripped = last.strip_suffix(b".html")?.to_vec();
            if stripped.is_empty() {
                return None;
            }
            *last = stripped;
            Some(Page::Blob(path))
        }
        [r, rest @ ..] if r == b"raw" && !rest.is_empty() => Some(Page::Raw(rest.to_vec())),
        _ => None,
    }
}

pub fn is_oid(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64)
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

type RenderFn = Box<dyn FnOnce(&mut dyn Out) -> Result<(), RenderError> + Send>;
type LoadFn = Box<dyn FnOnce() -> anyhow::Result<(Bytes, &'static str)> + Send>;

enum Body_ {
    /// Streamed page, rendered under a render permit.
    Render(RenderFn),
    /// Whole body loaded under a render permit (raw blobs: bounded memory,
    /// content type known only after loading).
    Deferred(LoadFn),
}

/// Everything about a page that is known before rendering (headers, cache key).
#[derive(Clone)]
struct PageHead {
    key: String,
    etag: String,
    content_type: &'static str,
    cache_control: &'static str,
    last_modified: Option<i64>,
    deadline: Instant,
}

struct Prepared {
    head: PageHead,
    body: Body_,
}

enum PageError {
    NotFound(String, Option<Box<Ctx>>),
    TooLarge(String, Option<Box<Ctx>>),
    Internal(anyhow::Error, Option<Box<Ctx>>),
}

fn meta_ctx(state: &AppState, meta: &crate::git::repo::Meta, relpath: &str) -> Ctx {
    let mut c = error_ctx(meta.clone(), relpath.to_string());
    c.page_size = state.cfg.page_size();
    c.feed_entries = state.cfg.feed_entries;
    c.max_files = state.cfg.max_files;
    c.files_line_counts = state.cfg.files_line_counts;
    c.max_blob_bytes = state.cfg.max_blob_bytes;
    c.limits = state.limits();
    c.link_refs = state.cfg.link_refs;
    c
}

/// Cheap phase: open the repo, resolve ids, decide key/ETag and build the render closure.
fn prepare(state: &Arc<AppState>, resolved: &Resolved, page: &Page) -> Result<Prepared, PageError> {
    let relpath = match page {
        Page::Commit(_) => "../".to_string(),
        Page::Blob(p) => "../".repeat(p.len()),
        _ => String::new(),
    };
    let repo = match state.repos.open(&resolved.repo.git_dir) {
        Ok(r) => r,
        Err(_) => {
            state.repos.evict(&resolved.repo.git_dir);
            state
                .repos
                .open(&resolved.repo.git_dir)
                .map_err(|e| PageError::Internal(e, None))?
        }
    };
    let mut meta = read_meta(&resolved.repo, state.cfg.name_from);
    let tip = match resolve_head(&repo) {
        Ok(t) => t,
        Err(e) => {
            return Err(PageError::Internal(
                e,
                Some(Box::new(meta_ctx(state, &meta, &relpath))),
            ))
        }
    };
    let tree = match tip {
        Some(t) => match repo.find_commit(t).and_then(|c| c.tree_id()) {
            Ok(id) => Some(id.detach()),
            Err(e) => {
                return Err(PageError::Internal(
                    e.into(),
                    Some(Box::new(meta_ctx(state, &meta, &relpath))),
                ))
            }
        },
        None => None,
    };
    if let Some(t) = tree {
        if let Err(e) = detect_links(&repo, t, &mut meta) {
            return Err(PageError::Internal(
                e,
                Some(Box::new(meta_ctx(state, &meta, &relpath))),
            ));
        }
    }
    let mut ctx = meta_ctx(state, &meta, &relpath);
    ctx.base_url = match &state.cfg.public_url {
        Some(u) => format!("{}{}/", u.trim_end_matches('/'), resolved.base),
        None => String::new(),
    };
    let timeout = state.cfg.render_timeout;
    // repositories can share tips (versions, forks); keys and ETags must be per repo
    let rh = format!(
        "{:08x}",
        xxhash_rust::xxh3::xxh3_64(resolved.repo.git_dir.as_os_str().as_encoded_bytes()) as u32
    );
    let tv = format!("{rh}{}", state.tv);
    let nf = |what: &str, ctx: Ctx| PageError::NotFound(what.to_string(), Some(Box::new(ctx)));
    let tip_s = tip
        .map(|t| t.to_string())
        .unwrap_or_else(|| "empty".to_string());
    let tree_s = tree
        .map(|t| t.to_string())
        .unwrap_or_else(|| "empty".to_string());
    let st = state.clone();

    match page {
        Page::Log { from, reference } => {
            let (start_tip, suffix) = match reference {
                Some(name) => match resolve_shorthand(&repo, name) {
                    Some(id) => (Some(id), format!("&ref={}", pct_str(name))),
                    None => return Err(nf("ref not found", ctx)),
                },
                None => (tip, String::new()),
            };
            let from_id = match from {
                Some(f) => {
                    let id = ObjectId::from_hex(f.as_bytes()).ok().filter(|_| is_oid(f));
                    match id {
                        Some(id) if crate::git::repo::is_commit(&repo, id) => Some(id),
                        _ => return Err(nf("commit not found", ctx)),
                    }
                }
                None => None,
            };
            let n = state.cfg.page_size();
            let start_s = start_tip
                .map(|t| t.to_string())
                .unwrap_or_else(|| "empty".to_string());
            let from_s = from_id.map(|t| t.to_string()).unwrap_or_default();
            let ref_s = reference.as_deref().unwrap_or("");
            let key = format!("{rh}:log:{start_s}:{from_s}:{n}:{ref_s}");
            // the ref name is part of the page (pagination links), not only its tip
            let ref_h = format!(
                "{:08x}",
                xxhash_rust::xxh3::xxh3_64(ref_s.as_bytes()) as u32
            );
            let etag = format!("\"{start_s}-{from_s}-{n}-{ref_h}-{tv}\"");
            let deadline = Instant::now() + timeout * 2;
            ctx.deadline = deadline;
            Ok(Prepared {
                head: PageHead {
                    key,
                    etag,
                    content_type: "text/html; charset=utf-8",
                    cache_control: "public, max-age=60, must-revalidate",
                    last_modified: None,
                    deadline,
                },
                body: Body_::Render(Box::new(move |out| {
                    crate::render::log::render_log(
                        &repo, &ctx, &st.stats, start_tip, from_id, &suffix, out,
                    )
                })),
            })
        }
        Page::Commit(hex) => {
            let id = ObjectId::from_hex(hex.as_bytes())
                .map_err(|_| nf("commit not found", ctx.clone()))?;
            // header only: the id may name a huge blob, which must not be loaded
            match repo.try_find_header(id) {
                Ok(Some(h)) if h.kind() == gix::object::Kind::Commit => {}
                Ok(_) => return Err(nf("commit not found", ctx)),
                Err(e) => return Err(PageError::Internal(e.into(), Some(Box::new(ctx)))),
            }
            let last_modified = repo.find_commit(id).ok().and_then(|c| {
                c.committer()
                    .ok()
                    .map(|c| crate::git::commit::parse_time(c.time.as_ref()).0)
            });
            let deadline = Instant::now() + timeout;
            ctx.deadline = deadline;
            Ok(Prepared {
                head: PageHead {
                    key: format!("{rh}:commit:{id}"),
                    etag: format!("\"{id}-{tv}\""),
                    content_type: "text/html; charset=utf-8",
                    cache_control: "public, max-age=86400",
                    last_modified,
                    deadline,
                },
                body: Body_::Render(Box::new(move |out| {
                    crate::render::commit::render_commit(&repo, &ctx, &st.stats, id, out)
                })),
            })
        }
        Page::Files => {
            let deadline = Instant::now() + timeout;
            ctx.deadline = deadline;
            Ok(Prepared {
                head: PageHead {
                    key: format!("{rh}:files:{tree_s}"),
                    etag: format!("\"{tree_s}-{tv}\""),
                    content_type: "text/html; charset=utf-8",
                    cache_control: "public, max-age=60, must-revalidate",
                    last_modified: None,
                    deadline,
                },
                body: Body_::Render(Box::new(move |out| {
                    crate::render::files::render_files(&repo, &ctx, &st.stats, tree, out)
                })),
            })
        }
        Page::Blob(path) | Page::Raw(path) => {
            let segs: Vec<&[u8]> = path.iter().map(|s| s.as_slice()).collect();
            let entry = match tree.map(|t| crate::git::tree::lookup_blob(&repo, t, &segs)) {
                Some(Ok(Some(e))) => e,
                Some(Ok(None)) | None => return Err(nf("file not found", ctx)),
                Some(Err(e)) => return Err(PageError::Internal(e, Some(Box::new(ctx)))),
            };
            let full_path = entry.path.clone();
            let id = entry.id;
            let deadline = Instant::now() + timeout;
            ctx.deadline = deadline;
            // gix materialises whole objects: refuse blobs that would not fit the
            // per-render memory budget before anything is loaded
            let size = repo
                .find_header(id)
                .map(|h| h.size())
                .map_err(|e| PageError::Internal(e.into(), Some(Box::new(ctx.clone()))))?;
            if size > state.cfg.max_commit_diff_bytes {
                return Err(PageError::TooLarge(
                    format!("file too large ({size} bytes)"),
                    Some(Box::new(ctx)),
                ));
            }
            if matches!(page, Page::Raw(_)) {
                return Ok(Prepared {
                    head: PageHead {
                        key: format!("{rh}:raw:{id}"),
                        etag: format!("\"{id}\""),
                        content_type: "application/octet-stream",
                        cache_control: "public, max-age=60, must-revalidate",
                        last_modified: None,
                        deadline,
                    },
                    body: Body_::Deferred(Box::new(move || {
                        let data = repo.find_object(id)?.data.clone();
                        let ct = if is_binary(&data) {
                            "application/octet-stream"
                        } else {
                            "text/plain; charset=utf-8"
                        };
                        Ok((Bytes::from(data), ct))
                    })),
                });
            }
            let ph = xxhash_rust::xxh3::xxh3_64(&full_path);
            Ok(Prepared {
                head: PageHead {
                    key: format!("{rh}:blob:{id}:{ph:016x}"),
                    etag: format!("\"{id}-{ph:016x}-{tv}\""),
                    content_type: "text/html; charset=utf-8",
                    cache_control: "public, max-age=60, must-revalidate",
                    last_modified: None,
                    deadline,
                },
                body: Body_::Render(Box::new(move |out| {
                    crate::render::blob::render_blob(&repo, &ctx, id, &full_path, out)
                })),
            })
        }
        Page::Refs | Page::Tags => {
            let h = crate::git::refs::refs_hash(&repo)
                .map_err(|e| PageError::Internal(e, Some(Box::new(ctx.clone()))))?;
            let is_tags = matches!(page, Page::Tags);
            let deadline = Instant::now() + timeout;
            ctx.deadline = deadline;
            Ok(Prepared {
                head: PageHead {
                    key: format!("{rh}:{}:{h:016x}", if is_tags { "tags" } else { "refs" }),
                    etag: format!("\"{h:016x}-{tv}\""),
                    content_type: if is_tags {
                        "application/atom+xml; charset=utf-8"
                    } else {
                        "text/html; charset=utf-8"
                    },
                    cache_control: "public, max-age=60, must-revalidate",
                    last_modified: None,
                    deadline,
                },
                body: Body_::Render(Box::new(move |out| {
                    let refs = crate::git::refs::list_refs(&repo, || ctx.check())?;
                    if is_tags {
                        crate::render::atom::render_tags(&repo, &ctx, &refs, out)
                    } else {
                        crate::render::refs::render_refs(&ctx, &refs, out)
                    }
                })),
            })
        }
        Page::Atom => {
            let deadline = Instant::now() + timeout;
            ctx.deadline = deadline;
            Ok(Prepared {
                head: PageHead {
                    key: format!("{rh}:atom:{tip_s}"),
                    etag: format!("\"{tip_s}-{tv}\""),
                    content_type: "application/atom+xml; charset=utf-8",
                    cache_control: "public, max-age=60, must-revalidate",
                    last_modified: None,
                    deadline,
                },
                body: Body_::Render(Box::new(move |out| {
                    crate::render::atom::render_atom(&repo, &ctx, tip, out)
                })),
            })
        }
    }
}

fn pct_str(s: &str) -> String {
    let mut v = crate::render::out::VecOut::default();
    let _ = v.pct(s.as_bytes());
    String::from_utf8_lossy(&v.0).into_owned()
}

fn etag_matches(headers: &axum::http::HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == etag)
}

fn with_headers(mut r: Response<Body>, p: &PageHead) -> Response<Body> {
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(p.content_type),
    );
    h.insert(
        header::ETAG,
        HeaderValue::from_str(&p.etag).unwrap_or(HeaderValue::from_static("\"x\"")),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(p.cache_control),
    );
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if let Some(t) = p.last_modified {
        if let Ok(v) = HeaderValue::from_str(&crate::render::time::http_date(t)) {
            h.insert(header::LAST_MODIFIED, v);
        }
    }
    r
}

fn full_response(head: &PageHead, bytes: Bytes, is_head: bool) -> Response<Body> {
    let len = bytes.len();
    let mut r = Response::new(if is_head {
        Body::empty()
    } else {
        Body::from(bytes)
    });
    r.headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    with_headers(r, head)
}

fn redirect(location: String) -> Response<Body> {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::FOUND;
    r.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(&location).unwrap_or(HeaderValue::from_static("/")),
    );
    r
}

fn text(status: StatusCode, body: &'static str, ct: &'static str) -> Response<Body> {
    let mut r = Response::new(Body::from(body));
    *r.status_mut() = status;
    r.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    r
}

/// A render permit: immediately if free, otherwise after waiting at most
/// `queue_timeout` with at most `max_queue` other waiters; `None` means 503.
async fn acquire_permit(state: &AppState) -> Option<tokio::sync::OwnedSemaphorePermit> {
    if let Ok(p) = state.permits.clone().try_acquire_owned() {
        return Some(p);
    }
    if state.waiting.load(Ordering::Relaxed) >= state.cfg.max_queue {
        state.metrics.queue_drops.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    state.waiting.fetch_add(1, Ordering::Relaxed);
    let permit = tokio::time::timeout(
        state.cfg.queue_timeout,
        state.permits.clone().acquire_owned(),
    )
    .await;
    state.waiting.fetch_sub(1, Ordering::Relaxed);
    match permit {
        Ok(Ok(p)) => Some(p),
        _ => {
            state.metrics.queue_drops.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

fn metrics_text(state: &AppState) -> String {
    let m = &state.metrics;
    let (entries, bytes, hits, misses) = state.render_cache.stats();
    format!(
        "# TYPE streagit_requests_total counter\nstreagit_requests_total {}\n\
         # TYPE streagit_renders_total counter\nstreagit_renders_total {}\n\
         # TYPE streagit_render_seconds_total counter\nstreagit_render_seconds_total {:.3}\n\
         # TYPE streagit_cache_hits_total counter\nstreagit_cache_hits_total {}\n\
         # TYPE streagit_not_modified_total counter\nstreagit_not_modified_total {}\n\
         # TYPE streagit_queue_drops_total counter\nstreagit_queue_drops_total {}\n\
         # TYPE streagit_render_timeouts_total counter\nstreagit_render_timeouts_total {}\n\
         # TYPE streagit_slow_clients_total counter\nstreagit_slow_clients_total {}\n\
         # TYPE streagit_render_errors_total counter\nstreagit_render_errors_total {}\n\
         # TYPE streagit_render_cache_entries gauge\nstreagit_render_cache_entries {}\n\
         # TYPE streagit_render_cache_bytes gauge\nstreagit_render_cache_bytes {}\n\
         # TYPE streagit_render_cache_lookups_total counter\nstreagit_render_cache_lookups_total{{result=\"hit\"}} {}\nstreagit_render_cache_lookups_total{{result=\"miss\"}} {}\n\
         # TYPE streagit_stats_entries gauge\nstreagit_stats_entries {}\n",
        m.requests.load(Ordering::Relaxed),
        m.renders.load(Ordering::Relaxed),
        m.render_millis.load(Ordering::Relaxed) as f64 / 1000.0,
        m.cache_hits.load(Ordering::Relaxed),
        m.not_modified.load(Ordering::Relaxed),
        m.queue_drops.load(Ordering::Relaxed),
        m.timeouts.load(Ordering::Relaxed),
        m.slow_clients.load(Ordering::Relaxed),
        m.errors.load(Ordering::Relaxed),
        entries,
        bytes,
        hits,
        misses,
        state.stats.len(),
    )
}

fn asset_response(
    headers: &axum::http::HeaderMap,
    asset: &super::assets::Asset,
    is_head: bool,
) -> Response<Body> {
    if etag_matches(headers, &asset.etag) {
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        r.headers_mut()
            .insert(header::ETAG, HeaderValue::from_str(&asset.etag).unwrap());
        return r;
    }
    let body = if is_head {
        Body::empty()
    } else {
        Body::from(asset.body.clone())
    };
    let mut r = Response::new(body);
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(asset.content_type),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(asset.body.len()));
    h.insert(header::ETAG, HeaderValue::from_str(&asset.etag).unwrap());
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    r
}

/// `--index`: the overview for `rest` (`/`, `/index.html`, `/<dir>/`,
/// `/<dir>/index.html`; `/<dir>` redirects). `None` when it is not an index path.
async fn index_page(
    state: &Arc<AppState>,
    headers: &axum::http::HeaderMap,
    is_head: bool,
    rest: &str,
) -> Option<Response<Body>> {
    let (segs, trailing) = split_path(rest).ok()?;
    let mut dir: Vec<String> = segs
        .into_iter()
        .map(String::from_utf8)
        .collect::<Result<_, _>>()
        .ok()?;
    let explicit = dir.last().is_some_and(|l| l == "index.html");
    if explicit {
        dir.pop();
    }
    let dir = dir.join("/");
    if !dir.is_empty() && state.excludes.is_excluded(&dir) {
        return None;
    }
    let entries = state.index_entries().await?;
    let body = crate::index::render(&entries, &dir)?;
    if !dir.is_empty() && !trailing && !explicit {
        return Some(redirect(format!("{}{}/", state.prefix, rest)));
    }
    let head = PageHead {
        key: String::new(),
        etag: format!(
            "\"idx-{:016x}-{}\"",
            xxhash_rust::xxh3::xxh3_64(&body),
            state.tv
        ),
        content_type: "text/html; charset=utf-8",
        cache_control: "public, max-age=60, must-revalidate",
        last_modified: None,
        deadline: Instant::now(),
    };
    if etag_matches(headers, &head.etag) {
        state.metrics.not_modified.fetch_add(1, Ordering::Relaxed);
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        return Some(with_headers(r, &head));
    }
    Some(full_response(&head, Bytes::from(body), is_head))
}

/// The one handler for everything under the prefix.
pub async fn handle(State(state): State<Arc<AppState>>, req: Request) -> Response<Body> {
    state.metrics.requests.fetch_add(1, Ordering::Relaxed);
    let method = req.method().clone();
    if method != Method::GET && method != Method::HEAD {
        let mut r = plain_page(
            StatusCode::METHOD_NOT_ALLOWED,
            "only GET and HEAD are supported",
        );
        r.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
        return r;
    }
    let path = req.uri().path().to_string();
    let Some(rest) = path.strip_prefix(state.prefix.as_str()) else {
        return plain_page(StatusCode::NOT_FOUND, "not under the configured prefix");
    };
    if !rest.is_empty() && !rest.starts_with('/') {
        return plain_page(StatusCode::NOT_FOUND, "not under the configured prefix");
    }
    match rest {
        "/healthz" => return text(StatusCode::OK, "ok\n", "text/plain; charset=utf-8"),
        "/robots.txt" => {
            let body = match state.cfg.robots {
                Robots::Allow => "User-agent: *\nAllow: /\n",
                Robots::Disallow => "User-agent: *\nDisallow: /\n",
            };
            return text(StatusCode::OK, body, "text/plain; charset=utf-8");
        }
        "/metrics" => {
            let mut r = Response::new(Body::from(metrics_text(&state)));
            r.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
            );
            return r;
        }
        "" if state.index.is_some() && !state.prefix.is_empty() => {
            return redirect(format!("{}/", state.prefix));
        }
        "" | "/" | "/index.html" if state.index.is_some() => {
            if let Some(r) = index_page(&state, req.headers(), method == Method::HEAD, "/").await {
                return r;
            }
        }
        "" | "/" => return plain_page(StatusCode::NOT_FOUND, "no index page"),
        _ => {}
    }
    // assets of the index pages
    if state.index.is_some() {
        if let Some(asset) = rest
            .strip_prefix('/')
            .and_then(|n| state.assets.get(n.as_bytes()))
        {
            return asset_response(req.headers(), asset, method == Method::HEAD);
        }
    }
    let (segs, _trailing) = match split_path(rest) {
        Ok(v) => v,
        Err(PathError::TooLong) => return plain_page(StatusCode::URI_TOO_LONG, "path too long"),
        Err(PathError::Bad) => return plain_page(StatusCode::NOT_FOUND, "malformed path"),
    };
    let raw: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let Some(resolved) = resolve(
        &state.root,
        &state.canonical_root,
        &segs,
        &raw,
        state.cfg.allow_git_paths,
        state.cfg.allow_bare_dirs(),
    )
    .filter(|r| !state.excluded(&r.repo)) else {
        if state.index.is_some() {
            if let Some(r) = index_page(&state, req.headers(), method == Method::HEAD, rest).await {
                return r;
            }
        }
        return plain_page(StatusCode::NOT_FOUND, "repository not found");
    };

    // repo root and index.html -> log.html
    if resolved.page.is_empty() || resolved.page == [b"index.html".to_vec()] {
        return redirect(format!("{}{}/log.html", state.prefix, resolved.base));
    }
    // per-repo assets
    if let [name] = resolved.page.as_slice() {
        if let Some(asset) = state.assets.get(name) {
            if etag_matches(req.headers(), &asset.etag) {
                state.metrics.not_modified.fetch_add(1, Ordering::Relaxed);
            }
            return asset_response(req.headers(), asset, method == Method::HEAD);
        }
    }
    let Some(page) = classify(&resolved.page, req.uri().query()) else {
        // unknown page under a known repo: 404 with the repo's header
        let st = state.clone();
        let rr = resolved.repo.clone();
        let ctx = tokio::task::spawn_blocking(move || {
            let meta = read_meta(&rr, st.cfg.name_from);
            meta_ctx(&st, &meta, "")
        })
        .await
        .ok();
        return match ctx {
            Some(ctx) => repo_page(StatusCode::NOT_FOUND, "no such page", &ctx),
            None => plain_page(StatusCode::NOT_FOUND, "no such page"),
        };
    };

    // phase 1: prepare (cheap, blocking pool, no render permit)
    let st = state.clone();
    let pg = page.clone();
    let prepared = tokio::task::spawn_blocking(move || prepare(&st, &resolved, &pg)).await;
    let Prepared { head, body } = match prepared {
        Ok(Ok(p)) => p,
        Ok(Err(PageError::NotFound(what, ctx))) => {
            return match ctx {
                Some(ctx) => repo_page(StatusCode::NOT_FOUND, &what, &ctx),
                None => plain_page(StatusCode::NOT_FOUND, &what),
            }
        }
        Ok(Err(PageError::TooLarge(what, ctx))) => {
            return match ctx {
                Some(ctx) => repo_page(StatusCode::PAYLOAD_TOO_LARGE, &what, &ctx),
                None => plain_page(StatusCode::PAYLOAD_TOO_LARGE, &what),
            }
        }
        Ok(Err(PageError::Internal(e, ctx))) => {
            state.metrics.errors.fetch_add(1, Ordering::Relaxed);
            tracing::error!(error = format!("{e:#}"), path, "prepare failed");
            return match ctx {
                Some(ctx) => repo_page(StatusCode::INTERNAL_SERVER_ERROR, "repository error", &ctx),
                None => plain_page(StatusCode::INTERNAL_SERVER_ERROR, "repository error"),
            };
        }
        Err(e) => {
            state.metrics.errors.fetch_add(1, Ordering::Relaxed);
            tracing::error!(error = %e, path, "prepare panicked");
            return plain_page(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };

    // phase 2: conditional request
    if etag_matches(req.headers(), &head.etag) {
        state.metrics.not_modified.fetch_add(1, Ordering::Relaxed);
        let mut r = Response::new(Body::empty());
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        return with_headers(r, &head);
    }
    let is_head = method == Method::HEAD;
    let render = match body {
        Body_::Deferred(load) => {
            // whole-body pages (raw blobs): loaded under a permit, never cached
            let Some(permit) = acquire_permit(&state).await else {
                return overloaded();
            };
            let loaded = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                load()
            })
            .await;
            return match loaded {
                Ok(Ok((bytes, content_type))) => {
                    let head = PageHead {
                        content_type,
                        ..head
                    };
                    full_response(&head, bytes, is_head)
                }
                Ok(Err(e)) => {
                    state.metrics.errors.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(error = format!("{e:#}"), path, "load failed");
                    plain_page(StatusCode::INTERNAL_SERVER_ERROR, "repository error")
                }
                Err(e) => {
                    state.metrics.errors.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(error = %e, path, "load panicked");
                    plain_page(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
                }
            };
        }
        Body_::Render(f) => f,
    };
    // phase 3: render cache
    if let Some(bytes) = state.render_cache.get(&head.key) {
        state.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
        return full_response(&head, bytes, is_head);
    }
    if is_head {
        // HEAD never renders: headers only (length unknown)
        return with_headers(Response::new(Body::empty()), &head);
    }
    // phase 4: single-flight
    let leader = match state.inflight.begin(&head.key) {
        Ticket::Leader(l) => Some(l),
        Ticket::Follower(rx) => {
            if let Some(bytes) = follow(rx).await {
                state.metrics.cache_hits.fetch_add(1, Ordering::Relaxed);
                return full_response(&head, bytes, is_head);
            }
            None
        }
    };
    // phase 5: render permit (at most `max_queue` requests wait, at most `queue_timeout`)
    let Some(permit) = acquire_permit(&state).await else {
        return overloaded();
    };
    if Instant::now() >= head.deadline {
        return overloaded();
    }
    // phase 6: stream
    let (tx, rx) = mpsc::channel::<Bytes>(CHANNEL_DEPTH);
    let r = Response::new(Body::from_stream(
        ReceiverStream::new(rx).map(Ok::<Bytes, std::convert::Infallible>),
    ));
    let r = with_headers(r, &head);
    let st = state.clone();
    let key = head.key.clone();
    let max_keep = state.render_cache.max_entry;
    let deadline = head.deadline;
    let path_for_log = path.clone();
    state.metrics.renders.fetch_add(1, Ordering::Relaxed);
    let path_for_panic = path.clone();
    let st_for_panic = state.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let started = Instant::now();
        let mut out = ChunkOut::new(tx, max_keep, deadline);
        let result = render(&mut out);
        let outcome = match result {
            Ok(()) => out.finish().ok().flatten(),
            Err(RenderError::Cancelled) => None,
            Err(RenderError::SlowClient) => {
                st.metrics.slow_clients.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    path = path_for_log,
                    "client did not read the response before the deadline; render aborted"
                );
                None
            }
            Err(RenderError::Timeout) => {
                st.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(path = path_for_log, "render timed out");
                out.uncacheable();
                let _ = out.write(b"<p>Rendering aborted: time limit exceeded.</p>\n");
                let _ = write_footer(&mut out);
                let _ = out.flush();
                None
            }
            Err(RenderError::Git(e)) => {
                st.metrics.errors.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    path = path_for_log,
                    error = format!("{e:#}"),
                    "render failed"
                );
                out.uncacheable();
                let _ = out.write(b"<p>Rendering aborted: repository error.</p>\n");
                let _ = write_footer(&mut out);
                let _ = out.flush();
                None
            }
        };
        st.metrics
            .render_millis
            .fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        if let Some(bytes) = &outcome {
            st.render_cache.put(key, bytes.clone());
        }
        if let Some(l) = leader {
            l.finish(outcome);
        }
    });
    // a panicking render drops the sender (the stream just ends) and the leader
    // (followers render themselves); log it so it does not go unnoticed
    tokio::spawn(async move {
        if let Err(e) = task.await {
            st_for_panic.metrics.errors.fetch_add(1, Ordering::Relaxed);
            tracing::error!(path = path_for_panic, error = %e, "render task panicked");
        }
    });
    r
}
