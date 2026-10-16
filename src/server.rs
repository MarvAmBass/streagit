//! Router, middleware stack and graceful shutdown (SPEC §3, §9).

use std::sync::Arc;

use axum::Router;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::http::handlers::{handle, AppState};

/// Build the application router for `state`.
pub fn router(state: Arc<AppState>) -> Router {
    let compression = state.cfg.compression;
    let max_connections = state.cfg.max_connections;
    let app = Router::new().fallback(handle).with_state(state);
    let app = if compression {
        app.layer(CompressionLayer::new().gzip(true))
    } else {
        app
    };
    app.layer(TraceLayer::new_for_http())
        .layer(ConcurrencyLimitLayer::new(max_connections))
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
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(listen = %listener.local_addr()?, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
