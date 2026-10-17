//! `--index`, suffix-less repositories, `--exclude`, basic auth and read-only /
//! foreign-owned repositories.

mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use base64::Engine;
use common::*;
use http_body_util::BodyExt;
use streagit::config::{Config, NameFrom};
use tower::ServiceExt;

fn index_config() -> Config {
    let mut cfg = Config::for_root(index_fixture_root().to_path_buf());
    cfg.compression = false;
    cfg.index = true;
    cfg
}

fn golden_index() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/index.html");
    String::from_utf8(std::fs::read(p).unwrap()).unwrap()
}

/// stagit-index's page for `c++ alpha beta.git`, compared byte for byte. The one
/// normalisation: stagit-index links `beta.git` as `beta/log.html` (the
/// directory stagit would write), streagit serves it at `beta.git/`.
#[tokio::test]
async fn index_matches_stagit_index() {
    let mut cfg = index_config();
    cfg.index_depth = 1;
    let (app, _) = app_with(cfg);
    let r = get(&app, "/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    let golden = golden_index().replace("\"beta/log.html\"", "\"beta.git/log.html\"");
    assert_eq!(r.text(), golden);
    // index.html is the same page
    assert_eq!(get(&app, "/index.html").await.text(), golden);
}

fn links(page: &str) -> Vec<String> {
    page.split("<tr><td><a href=\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn index_lists_nested_repositories_newest_first() {
    let mut cfg = index_config();
    cfg.exclude = vec!["mirrors".into()];
    let (app, _) = app_with(cfg);
    let r = get(&app, "/").await;
    assert_eq!(r.status, StatusCode::OK);
    let page = r.text();
    assert_eq!(
        links(&page),
        [
            "team/tool.git/log.html",
            "c%2B%2B/log.html",
            "team/app/log.html",
            "alpha/log.html",
            "beta.git/log.html",
            "team/empty.git/log.html",
        ],
        "{page}"
    );
    assert!(page
        .contains(">team/tool</a></td><td>A tool</td><td>Team</td><td>2024-07-03 09:46</td></tr>"));
    assert!(page.contains(">team/empty</a></td><td>Nothing yet</td><td></td><td></td></tr>"));
    // every link is a working repository page
    for l in links(&page) {
        let r = get(&app, &format!("/{l}")).await;
        assert_html(&r);
    }
    // ETag / 304
    let etag = r.header("etag").unwrap().to_string();
    let r = request(&app, "GET", "/", &[("if-none-match", &etag)]).await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    // a deeper walk finds deep/a/b/c.git
    let mut cfg = index_config();
    cfg.index_depth = 4;
    let (app, _) = app_with(cfg);
    let page = get(&app, "/").await.text();
    assert!(page.contains("href=\"deep/a/b/c.git/log.html\""));
    assert!(page.contains("href=\"mirrors/m.git/log.html\""));
    assert!(!page.contains("links/"));
}

#[tokio::test]
async fn index_under_a_prefix_and_per_directory() {
    let mut cfg = index_config();
    cfg.prefix = "/browse".into();
    cfg.exclude = vec!["mirrors".into()];
    let (app, _) = app_with(cfg);
    let r = get(&app, "/browse").await;
    assert_eq!(r.status, StatusCode::FOUND);
    assert_eq!(r.header("location"), Some("/browse/"));
    assert_eq!(get(&app, "/browse/").await.status, StatusCode::OK);
    assert_eq!(get(&app, "/browse/index.html").await.status, StatusCode::OK);
    for asset in ["style.css", "logo.png", "favicon.png"] {
        let r = get(&app, &format!("/browse/{asset}")).await;
        assert_eq!(r.status, StatusCode::OK, "{asset}");
    }
    // the logo on a repository page links ../ -> the directory's own index
    let r = get(&app, "/browse/team/tool.git/log.html").await;
    assert!(r.text().contains("<a href=\"../\"><img src=\"logo.png\""));
    let r = get(&app, "/browse/team/").await;
    assert_eq!(r.status, StatusCode::OK);
    let page = r.text();
    assert_eq!(
        links(&page),
        ["tool.git/log.html", "app/log.html", "empty.git/log.html"]
    );
    assert!(page.contains("href=\"../style.css\""));
    assert!(page.contains(">tool</a>"));
    let r = get(&app, "/browse/team").await;
    assert_eq!(r.header("location"), Some("/browse/team/"));
    assert_eq!(
        get(&app, "/browse/team/index.html").await.status,
        StatusCode::OK
    );
    for path in [
        "/browse/mirrors/",
        "/browse/mirrors/m.git/log.html",
        "/browse/nothing/",
        "/browse/team/nothing/",
        "/browse/team/tool.git/nosuch/",
        "/browse/links/",
    ] {
        assert_eq!(
            get(&app, path).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
}

#[tokio::test]
async fn suffixless_repositories_and_naming() {
    // off by default (no --index): only .git names are served
    let mut cfg = index_config();
    cfg.index = false;
    let (app, _) = app_with(cfg.clone());
    assert_eq!(get(&app, "/").await.status, StatusCode::NOT_FOUND);
    assert_eq!(get(&app, "/style.css").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&app, "/alpha/log.html").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/team/app/log.html").await.status,
        StatusCode::NOT_FOUND
    );
    assert_html(&get(&app, "/team/tool.git/log.html").await);
    // explicitly on
    cfg.allow_bare_dirs = Some(true);
    let (app, _) = app_with(cfg.clone());
    let r = get(&app, "/team/app/log.html").await;
    assert_html(&r);
    // default naming is the slot (the parent directory) – plain layouts want dir or path
    assert!(r.text().contains("<h1>team</h1>"), "{}", r.text());
    let r = get(&app, "/team/app/").await;
    assert_eq!(r.header("location"), Some("/team/app/log.html"));
    let r = get(&app, "/c%2B%2B/files.html").await;
    assert_html(&r);
    for (nf, a, b) in [
        (NameFrom::Dir, "app", "tool"),
        (NameFrom::Path, "team/app", "team/tool"),
    ] {
        cfg.name_from = nf;
        let (app, _) = app_with(cfg.clone());
        let r = get(&app, "/team/app/log.html").await;
        assert!(r.text().contains(&format!("<h1>{a}</h1>")), "{}", r.text());
        let r = get(&app, "/team/tool.git/log.html").await;
        assert!(r.text().contains(&format!("<h1>{b}</h1>")), "{}", r.text());
    }
    // can be switched off while the index is on (then only .git repos are listed)
    let mut cfg = index_config();
    cfg.allow_bare_dirs = Some(false);
    let (app, _) = app_with(cfg);
    let page = get(&app, "/").await.text();
    assert!(!page.contains("alpha/log.html") && page.contains("beta.git/log.html"));
    assert_eq!(
        get(&app, "/alpha/log.html").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn excluded_repositories_are_not_served() {
    let mut cfg = index_config();
    cfg.index = false;
    cfg.exclude = vec!["mirrors".into(), "team/tool.git".into()];
    let (app, _) = app_with(cfg.clone());
    for path in [
        "/mirrors/m.git/log.html",
        "/mirrors/m.git/raw/README",
        "/team/tool.git/log.html",
        "/links/team/tool.git/log.html", // via a symlink: the real path counts too
    ] {
        assert_eq!(
            get(&app, path).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_html(&get(&app, "/beta.git/log.html").await);
    assert_html(&get(&app, "/links/team/empty.git/log.html").await);
    // globs; the requested (symlink) path counts as well
    cfg.exclude = vec!["*/empty.git".into(), "links".into()];
    let (app, _) = app_with(cfg);
    assert_eq!(
        get(&app, "/team/empty.git/log.html").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/links/team/tool.git/log.html").await.status,
        StatusCode::NOT_FOUND
    );
    assert_html(&get(&app, "/team/tool.git/log.html").await);
}

fn basic(user: &str, pass: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
    )
}

fn auth_app(
    dir: &Path,
) -> (
    axum::Router,
    std::sync::Arc<streagit::http::handlers::AppState>,
) {
    let f = dir.join("users");
    std::fs::write(&f, "# users\nalice:wonder land\nbob:builder\n").unwrap();
    let mut cfg = index_config();
    cfg.prefix = "/browse".into();
    cfg.auth_file = Some(f);
    app_with(cfg)
}

async fn request_from(app: &axum::Router, path: &str, ip: &str, auth: Option<&str>) -> Resp {
    let mut req = Request::builder().uri(path);
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let mut req = req.body(Body::empty()).unwrap();
    let addr: SocketAddr = format!("{ip}:1234").parse().unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    let resp = app.clone().oneshot(req).await.unwrap();
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

#[tokio::test]
async fn basic_auth_guards_everything_but_healthz() {
    let dir = tempfile::tempdir().unwrap();
    let (app, state) = auth_app(dir.path());
    assert_eq!(state.auth.as_ref().unwrap().user_count(), 2);
    for path in [
        "/browse/",
        "/browse/style.css",
        "/browse/team/tool.git/log.html",
        "/browse/team/tool.git/raw/README",
        "/browse/team/tool.git/style.css",
        "/browse/robots.txt",
        "/browse/metrics",
        "/browse/nothing/here",
        "/elsewhere",
    ] {
        let r = get(&app, path).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(
            r.header("www-authenticate"),
            Some("Basic realm=\"streagit\", charset=\"UTF-8\""),
            "{path}"
        );
        let r = request(
            &app,
            "GET",
            path,
            &[("authorization", &basic("alice", "wrong"))],
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{path}");
        let r = request(
            &app,
            "GET",
            path,
            &[("authorization", &basic("bob", "builder"))],
        )
        .await;
        assert_ne!(r.status, StatusCode::UNAUTHORIZED, "{path}");
        assert!(
            r.header("cache-control").unwrap().starts_with("private"),
            "{path}: {:?}",
            r.header("cache-control")
        );
    }
    assert_eq!(get(&app, "/browse/healthz").await.status, StatusCode::OK);
    let ok = basic("alice", "wonder land");
    let r = request(
        &app,
        "GET",
        "/browse/team/tool.git/log.html",
        &[("authorization", &ok)],
    )
    .await;
    assert_html(&r);
    assert_eq!(
        r.header("cache-control"),
        Some("private, max-age=60, must-revalidate")
    );
    // conditional requests still work behind auth
    let etag = r.header("etag").unwrap().to_string();
    let r = request(
        &app,
        "GET",
        "/browse/team/tool.git/log.html",
        &[("authorization", &ok), ("if-none-match", &etag)],
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    assert!(r.header("cache-control").unwrap().starts_with("private"));
    // without auth configured nothing changes
    let (app, _) = app_with(index_config());
    let r = get(&app, "/team/tool.git/log.html").await;
    assert_eq!(
        r.header("cache-control"),
        Some("public, max-age=60, must-revalidate")
    );
}

#[tokio::test]
async fn failed_logins_lock_out_the_address() {
    let dir = tempfile::tempdir().unwrap();
    let (app, _) = auth_app(dir.path());
    let ok = basic("bob", "builder");
    let bad = basic("bob", "nope");
    let page = "/browse/beta.git/log.html";
    for _ in 0..streagit::auth::MAX_FAILURES {
        let r = request_from(&app, page, "198.51.100.7", Some(&bad)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    let r = request_from(&app, page, "198.51.100.7", Some(&ok)).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(r.header("retry-after").is_some());
    // other clients are not affected, health checks stay up
    let r = request_from(&app, page, "198.51.100.8", Some(&ok)).await;
    assert_eq!(r.status, StatusCode::OK);
    let r = request_from(&app, "/browse/healthz", "198.51.100.7", None).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn startup_fails_on_a_bad_auth_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = index_config();
    cfg.auth_file = Some(dir.path().join("missing"));
    assert!(streagit::http::handlers::AppState::new(cfg.clone()).is_err());
    let f = dir.path().join("empty");
    std::fs::write(&f, "\n# nobody\n").unwrap();
    cfg.auth_file = Some(f);
    assert!(streagit::http::handlers::AppState::new(cfg.clone()).is_err());
    // the environment pair
    cfg.auth_file = None;
    cfg.auth_user = Some("u".into());
    assert!(streagit::http::handlers::AppState::new(cfg.clone()).is_err());
    cfg.auth_password = Some(streagit::config::Secret("p".into()));
    let st = streagit::http::handlers::AppState::new(cfg).unwrap();
    assert_eq!(st.auth.unwrap().user_count(), 1);
}

fn sh(cmd: &str, dir: &Path) {
    let out = std::process::Command::new("sh")
        .arg("-ec")
        .arg(cmd)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{cmd}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// streagit never writes to repositories: everything works on a tree without
/// write permission and – when the tests run as root, e.g. in Docker – owned
/// by another user (gix then opens the repositories with reduced trust).
#[tokio::test]
async fn read_only_and_foreign_owned_repositories() {
    let tmp = tempfile::tempdir().unwrap();
    let root: PathBuf = tmp.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let src = index_fixture_root();
    sh(
        &format!("cp -a '{}/.' '{}/'", src.display(), root.display()),
        tmp.path(),
    );
    let main_repo = fixture_root().join("github.com/alice/repos/fixture/repo.git");
    sh(
        &format!(
            "cp -a '{}' '{}/fixture.git'",
            main_repo.display(),
            root.display()
        ),
        tmp.path(),
    );
    // a blob above gix's 16 MiB reduced-trust allocation limit
    sh(
        "git init -q --bare big.git \
         && awk 'BEGIN { for (i = 0; i < 400000; i++) print \"line \" i \" of a big file, padded to be long enough\" }' > big.txt \
         && id=$(git -C big.git hash-object -w ../big.txt) \
         && tree=$(printf '100644 blob %s\\tbig.txt\\n' \"$id\" | git -C big.git mktree) \
         && c=$(GIT_AUTHOR_NAME=a GIT_AUTHOR_EMAIL=a@b GIT_COMMITTER_NAME=a GIT_COMMITTER_EMAIL=a@b git -C big.git commit-tree -m big \"$tree\") \
         && git -C big.git update-ref refs/heads/main \"$c\" && rm big.txt",
        &root,
    );
    let big_size = 400_000 * 50;
    let as_root = unsafe { libc::geteuid() } == 0;
    if as_root {
        sh("chown -R 65534:65534 .", &root);
    }
    sh("chmod -R a-w .", &root);
    let marker = tmp.path().join("marker");
    std::fs::write(&marker, "").unwrap();

    let mut cfg = index_config();
    cfg.root = root.clone();
    cfg.prefix = "/browse".into();
    cfg.files_line_counts = true;
    let cache = tmp.path().join("cache");
    cfg.cache_dir = Some(cache.clone());
    let (app, _) = app_with(cfg);
    let page = get(&app, "/browse/").await.text();
    assert!(page.contains("href=\"fixture.git/log.html\""), "{page}");
    assert!(page.contains("href=\"team/app/log.html\""), "{page}");
    let id = git(&["rev-parse", "HEAD"]).trim().to_string();
    for p in [
        "/browse/fixture.git/log.html".to_string(),
        "/browse/fixture.git/files.html".to_string(),
        "/browse/fixture.git/refs.html".to_string(),
        format!("/browse/fixture.git/commit/{id}.html"),
        "/browse/fixture.git/file/README.md.html".to_string(),
        "/browse/team/app/log.html".to_string(),
        "/browse/big.git/log.html".to_string(),
        "/browse/big.git/file/big.txt.html".to_string(),
    ] {
        assert_html(&get(&app, &p).await);
    }
    for p in [
        "/browse/fixture.git/atom.xml",
        "/browse/fixture.git/tags.xml",
    ] {
        assert_eq!(get(&app, p).await.status, StatusCode::OK, "{p}");
    }
    let r = get(&app, "/browse/big.git/raw/big.txt").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.body.len() >= big_size, "{}", r.body.len());
    // nothing changed below the root
    let out = std::process::Command::new("find")
        .arg(&root)
        .args(["-newer", marker.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    sh("chmod -R u+w .", &root);
}
