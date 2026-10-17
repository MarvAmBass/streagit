//! Router, middleware stack and graceful shutdown (SPEC §3, §9).

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, Response, StatusCode};
use axum::middleware::Next;
use axum::Router;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::auth::Verdict;
use crate::config::Config;
use crate::http::errors::plain_page;
use crate::http::handlers::{handle, AppState};

/// Build the application router for `state`.
pub fn router(state: Arc<AppState>) -> Router {
    let compression = state.cfg.compression;
    let max_connections = state.cfg.max_connections;
    let with_auth = state.auth.is_some();
    let app = Router::new().fallback(handle).with_state(state.clone());
    let app = if compression {
        app.layer(CompressionLayer::new().gzip(true))
    } else {
        app
    };
    let app = if with_auth {
        app.layer(axum::middleware::from_fn_with_state(state, require_auth))
    } else {
        app
    };
    app.layer(TraceLayer::new_for_http())
        .layer(ConcurrencyLimitLayer::new(max_connections))
}

/// Basic auth for every request except `<prefix>/healthz`; responses become
/// `Cache-Control: private`.
async fn require_auth(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response<Body> {
    let Some(auth) = state.auth.as_ref() else {
        return next.run(req).await;
    };
    if req.uri().path().strip_prefix(state.prefix.as_str()) == Some("/healthz") {
        return next.run(req).await;
    }
    let ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let verdict = auth.check(
        ip,
        req.headers()
            .get(header::AUTHORIZATION)
            .map(|v| v.as_bytes()),
    );
    let mut r = match verdict {
        Verdict::Ok => next.run(req).await,
        Verdict::Denied => {
            let mut r = plain_page(StatusCode::UNAUTHORIZED, "authentication required");
            r.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"streagit\", charset=\"UTF-8\""),
            );
            r
        }
        Verdict::LockedOut(secs) => {
            let mut r = plain_page(
                StatusCode::TOO_MANY_REQUESTS,
                "too many failed logins, try again later",
            );
            r.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
            r
        }
    };
    let cc = match r
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
    {
        Some(v) if v.contains("private") => None,
        Some(v) => Some(match v.strip_prefix("public") {
            Some(rest) => format!("private{rest}"),
            None => format!("private, {v}"),
        }),
        None => Some("private".to_string()),
    };
    if let Some(cc) = cc.and_then(|c| HeaderValue::from_str(&c).ok()) {
        r.headers_mut().insert(header::CACHE_CONTROL, cc);
    }
    r
}

/// `setpriority(PRIO_PROCESS, 0, n)`.
pub fn set_nice(n: i32) {
    // SAFETY: plain syscall wrapper with constant arguments.
    let r = unsafe { libc::setpriority(libc::PRIO_PROCESS as _, 0, n) };
    if r != 0 {
        tracing::warn!(
            nice = n,
            "setpriority failed: {}",
            std::io::Error::last_os_error()
        );
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}

/// Run the server until SIGINT/SIGTERM.
pub async fn run(cfg: Config) -> anyhow::Result<()> {
    let listen = cfg.listen.clone();
    let state = Arc::new(AppState::new(cfg)?);
    tracing::info!(
        root = %state.root.display(),
        prefix = %state.prefix,
        render_threads = state.cfg.render_threads(),
        page_size = state.cfg.page_size(),
        "streagit {} starting",
        env!("CARGO_PKG_VERSION")
    );
    match &state.auth {
        Some(a) => tracing::info!(users = a.user_count(), "basic auth enabled"),
        None => tracing::info!("no authentication configured"),
    }
    if state.cfg.index {
        tracing::info!(
            depth = state.cfg.index_depth,
            ttl = ?state.cfg.index_ttl,
            "index page enabled"
        );
    }
    if !state.cfg.exclude.is_empty() {
        tracing::info!(patterns = state.cfg.exclude.len(), "excluding paths");
    }
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(listen = %listener.local_addr()?, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}
