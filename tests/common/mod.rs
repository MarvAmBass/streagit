//! Shared test harness: builds the fixture once and drives the router in-process.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use streagit::config::Config;
use streagit::http::handlers::AppState;
use tower::ServiceExt;

pub const SLOT: &str = "/github.com/alice/repos/fixture";

/// The fixture data root, built once per test binary with `tests/fixtures/make_fixture.sh`.
pub fn fixture_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let base = std::env::temp_dir().join(format!("streagit-fixture-{}", std::process::id()));
        let root = base.join("root");
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/make_fixture.sh");
        let out = std::process::Command::new("sh")
            .arg(&script)
            .arg(&root)
            .output()
            .expect("run make_fixture.sh (needs sh and git)");
        assert!(
            out.status.success(),
            "fixture build failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        root
    })
}

/// Run `git` inside the fixture's main repo and return stdout.
pub fn git(args: &[&str]) -> String {
    let dir = fixture_root().join("github.com/alice/repos/fixture/repo.git");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

pub fn config() -> Config {
    let mut cfg = Config::for_root(fixture_root().to_path_buf());
    cfg.prefix = "/browse".into();
    cfg.page_size = 10;
    cfg.compression = false;
    cfg
}

pub fn app_with(cfg: Config) -> (Router, Arc<AppState>) {
    let state = Arc::new(AppState::new(cfg).expect("state"));
    (streagit::server::router(state.clone()), state)
}

pub fn app() -> (Router, Arc<AppState>) {
    app_with(config())
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

pub async fn request(app: &Router, method: &str, path: &str, headers: &[(&str, &str)]) -> Resp {
    let mut req = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Resp {
        status,
        headers,
        body,
    }
}

pub async fn get(app: &Router, path: &str) -> Resp {
    request(app, "GET", path, &[]).await
}

pub fn assert_html(r: &Resp) {
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(
        r.header(header::CONTENT_TYPE.as_str()),
        Some("text/html; charset=utf-8")
    );
    let t = r.text();
    assert!(t.starts_with("<!DOCTYPE html>\n<html>\n"), "{t}");
    assert!(t.ends_with("</div>\n</body>\n</html>\n"), "{t}");
}
