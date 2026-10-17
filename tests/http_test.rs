//! HTTP-level tests against the fixture (SPEC §12.3).

mod common;

use axum::http::StatusCode;
use common::*;

fn commit_by_subject(subject: &str) -> String {
    let id = git(&[
        "log",
        "--all",
        "--format=%H",
        "-n1",
        "-F",
        "--grep",
        subject,
    ]);
    let id = id.trim().to_string();
    assert_eq!(id.len(), 40, "commit {subject:?} not found");
    id
}

#[tokio::test]
async fn resolution_table() {
    let (app, _) = app();
    for (path, name) in [
        (
            "/browse/github.com/alice/repos/fixture/html/log.html",
            "Fixture repository &amp; more",
        ),
        (
            "/browse/github.com/alice/repos/fixture/html.v1/log.html",
            "Fixture repository (version 1)",
        ),
        (
            "/browse/github.com/alice/repos/fixture/html.wiki/log.html",
            "Fixture wiki",
        ),
        (
            "/browse/github.com/alice/repos/fixture/forks/bob/html/log.html",
            "Bob&#39;s fork",
        ),
        (
            "/browse/github.com/alice/repos/fixture/forks/bob/html.v1/log.html",
            "Bob&#39;s fork (version 1)",
        ),
        (
            "/browse/github.com/alice/repos/fixture/forks/bob/repo.git/log.html",
            "Bob&#39;s fork",
        ),
        (
            "/browse/github.com/alice/repos/html/html/log.html",
            "A repo named html",
        ),
        (
            "/browse/github.com/alice/gists/abc123/html/log.html",
            "A gist",
        ),
        (
            "/browse/github.com/carol/stars/alice/fixture/html/log.html",
            "Fixture repository &amp; more",
        ),
    ] {
        let r = get(&app, path).await;
        assert_html(&r);
        assert!(
            r.text().contains(name),
            "{path}: expected {name:?} in\n{}",
            r.text()
        );
    }
}

#[tokio::test]
async fn names_follow_the_slot() {
    let (app, _) = app();
    let r = get(&app, "/browse/github.com/alice/repos/fixture/html/log.html").await;
    assert!(r.text().contains("<h1>fixture</h1>"));
    assert!(r
        .text()
        .contains("<title>Log - fixture - Fixture repository &amp; more</title>"));
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/forks/bob/html/log.html",
    )
    .await;
    assert!(r.text().contains("<h1>bob</h1>"));
    // --name-from owner-slot
    let mut cfg = config();
    cfg.name_from = streagit::config::NameFrom::OwnerSlot;
    let (app, _) = app_with(cfg);
    let r = get(&app, "/browse/github.com/alice/repos/fixture/html/log.html").await;
    assert!(r.text().contains("<h1>alice/fixture</h1>"), "{}", r.text());
}

#[tokio::test]
async fn not_found_and_traversal() {
    let (app, _) = app();
    for path in [
        "/browse/github.com/alice/repos/nothing/html/log.html",
        "/browse/github.com/alice/repos/evil/repo.git/log.html",
        "/browse/github.com/alice/repos/fixture/html/../html/log.html",
        "/browse/github.com/alice/repos/fixture/html/%2e%2e/log.html",
        "/browse/github.com%2Falice/repos/fixture/html/log.html",
        "/browse/github.com/alice/repos/fixture/html/log.html%00",
        "/browse/github.com/alice/repos/fixture/html.v9/log.html",
        "/browse/github.com/alice/repos/fixture/repo.git/../repo.git/log.html",
        "/browse/../etc/passwd",
        "/browse/",
        "/other/github.com/alice/repos/fixture/html/log.html",
        "/browse/github.com/alice/repos/fixture/html/nosuchpage.html",
        "/browse/github.com/alice/repos/fixture/html/commit/zzzz.html",
        "/browse/github.com/alice/repos/fixture/html/commit/0000000000000000000000000000000000000000.html",
        "/browse/github.com/alice/repos/fixture/html/file/nope.html",
        "/browse/github.com/alice/repos/fixture/html/file/src.html",
        "/browse/github.com/alice/repos/fixture/html/file/vendor/lib.html",
        "/browse/github.com/alice/repos/fixture/html/log.html?from=abc",
        "/browse/github.com/alice/repos/fixture/html/log.html?from=0000000000000000000000000000000000000000",
        "/browse/github.com/alice/repos/fixture/html/log.html?ref=nope",
    ] {
        let r = get(&app, path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}\n{}", r.text());
    }
    // direct .git paths can be switched off
    let mut cfg = config();
    cfg.allow_git_paths = false;
    let (app, _) = app_with(cfg);
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/repo.git/log.html",
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // too long
    let (app, _) = self::app();
    let long = format!("/browse/{}", "a/".repeat(3000));
    assert_eq!(get(&app, &long).await.status, StatusCode::URI_TOO_LONG);
}

#[tokio::test]
async fn unknown_page_uses_repo_header() {
    let (app, _) = app();
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/nosuchpage.html",
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.text().contains("<h1>fixture</h1>"));
    assert!(r.text().contains("<p>404 Not Found: no such page</p>"));
}

#[tokio::test]
async fn redirects_and_misc_endpoints() {
    let (app, _) = app();
    let r = get(&app, "/browse/github.com/alice/repos/fixture/html/").await;
    assert_eq!(r.status, StatusCode::FOUND);
    assert_eq!(
        r.header("location"),
        Some("/browse/github.com/alice/repos/fixture/html/log.html")
    );
    let r = get(&app, "/browse/github.com/alice/repos/fixture/html").await;
    assert_eq!(
        r.header("location"),
        Some("/browse/github.com/alice/repos/fixture/html/log.html")
    );
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html.v1/index.html",
    )
    .await;
    assert_eq!(
        r.header("location"),
        Some("/browse/github.com/alice/repos/fixture/html.v1/log.html")
    );
    let r = get(&app, "/browse/healthz").await;
    assert_eq!((r.status, r.text().as_str()), (StatusCode::OK, "ok\n"));
    let r = get(&app, "/browse/robots.txt").await;
    assert_eq!(r.text(), "User-agent: *\nDisallow: /\n");
    let r = get(&app, "/browse/metrics").await;
    assert!(r.text().contains("streagit_requests_total"));
    let r = request(
        &app,
        "POST",
        "/browse/github.com/alice/repos/fixture/html/log.html",
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.header("allow"), Some("GET, HEAD"));
}

#[tokio::test]
async fn assets_are_served_per_repo() {
    let (app, _) = app();
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/style.css",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("text/css; charset=utf-8"));
    assert!(r.text().contains("#blob"));
    let etag = r.header("etag").unwrap().to_string();
    let r = request(
        &app,
        "GET",
        "/browse/github.com/alice/repos/fixture/html/logo.png",
        &[("if-none-match", "\"nope\"")],
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("image/png"));
    let r = request(
        &app,
        "GET",
        "/browse/github.com/alice/repos/fixture/html/style.css",
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    // commit-level pages reference ../style.css which resolves to the same prefix
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/favicon.png",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    // assets don't exist without a repo
    let r = get(
        &app,
        "/browse/github.com/alice/repos/nothing/html/style.css",
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // --assets-dir override
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("style.css"), "body{color:red}").unwrap();
    let mut cfg = config();
    cfg.assets_dir = Some(dir.path().to_path_buf());
    let (app, _) = app_with(cfg);
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/style.css",
    )
    .await;
    assert_eq!(r.text(), "body{color:red}");
}

#[tokio::test]
async fn log_paging_covers_the_whole_history() {
    let (app, _) = app();
    let expected: Vec<String> = git(&["log", "--format=%H", "main"])
        .lines()
        .map(str::to_string)
        .collect();
    let all = expected.clone();
    let mut seen = Vec::new();
    let mut rows = 0;
    let mut url = "/browse/github.com/alice/repos/fixture/html/log.html".to_string();
    let mut pages = 0;
    loop {
        let r = get(&app, &url).await;
        assert_html(&r);
        let t = r.text();
        pages += 1;
        let ids: Vec<String> = t
            .match_indices("<a href=\"commit/")
            .map(|(i, _)| t[i + 16..i + 56].to_string())
            .collect();
        assert!(ids.len() <= 10);
        rows += t.matches("<tr><td>20").count();
        seen.extend(ids);
        match t.find("log.html?from=") {
            Some(i) => {
                let next = &t[i + 14..i + 54];
                assert!(t.contains("Next 10 commits &rarr;"));
                url = format!("/browse/github.com/alice/repos/fixture/html/log.html?from={next}");
                assert!(t.contains("&uarr; newest") == (pages > 1));
            }
            None => break,
        }
    }
    assert_eq!(
        seen, expected,
        "paging must reproduce git log order without gaps or duplicates"
    );
    assert_eq!(rows, all.len());
    assert_eq!(pages, all.len().div_ceil(10));
    // the rows carry diffstat numbers
    let r = get(&app, "/browse/github.com/alice/repos/fixture/html/log.html").await;
    assert!(r
        .text()
        .contains("<td class=\"num\" align=\"right\">1</td><td class=\"num\" align=\"right\">+"));
}

/// Follow `Next` links from page 1 and return every commit id in order.
async fn page_through(app: &axum::Router, base: &str) -> Vec<String> {
    let mut seen = Vec::new();
    let mut url = format!("{base}/log.html");
    for _ in 0..1000 {
        let r = get(app, &url).await;
        assert_html(&r);
        let t = r.text();
        seen.extend(
            t.match_indices("<a href=\"commit/")
                .map(|(i, _)| t[i + 16..i + 56].to_string()),
        );
        match t.find("log.html?from=") {
            Some(i) => url = format!("{base}/log.html?from={}", &t[i + 14..i + 54]),
            None => return seen,
        }
    }
    panic!("paging never ends");
}

#[tokio::test]
async fn log_paging_is_continuous_across_merges() {
    // the walk pops `x2` before `y2`, `y1`; `x1` is only reachable through `x2`.
    // A page boundary anywhere in between must not lose it.
    let expected: Vec<String> = git_in(
        "github.com/alice/repos/paging/repo.git",
        &["log", "--format=%H", "main"],
    )
    .lines()
    .map(str::to_string)
    .collect();
    assert_eq!(expected.len(), 7);
    for page_size in 1..=7 {
        let mut cfg = config();
        cfg.page_size = page_size;
        let (app, _) = app_with(cfg);
        let seen = page_through(&app, "/browse/github.com/alice/repos/paging/html").await;
        assert_eq!(seen, expected, "page size {page_size}");
    }
    // a ?from= that is not reachable from the tip still renders (starting there)
    let x1 = &expected[5];
    let r = get(
        &app().0,
        &format!("/browse/github.com/alice/repos/paging/html/log.html?from={x1}"),
    )
    .await;
    assert_html(&r);
    assert!(r.text().contains(&format!("commit/{x1}.html")));
}

#[tokio::test]
async fn bad_ids_and_names_are_not_found() {
    let (app, _) = app();
    let base = "/browse/github.com/alice/repos/fixture/html";
    let blob = git(&["rev-parse", "main:README.md"]);
    let tree = git(&["rev-parse", "main^{tree}"]);
    for path in [
        format!("{base}/log.html?ref={}", "a".repeat(1000)),
        format!("{base}/log.html?ref=refs/heads/main"),
        format!("{base}/log.html?ref=..%2F..%2Fconfig"),
        format!("{base}/log.html?from="),
        format!("{base}/log.html?from=..%2F..%2Fetc%2Fpasswd"),
        format!("{base}/log.html?from={}", blob.trim()),
        format!("{base}/commit/{}.html", blob.trim()),
        format!("{base}/commit/{}.html", tree.trim()),
    ] {
        let r = get(&app, &path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}\n{}", r.text());
    }
}

#[tokio::test]
async fn oversized_blobs_are_refused_before_loading() {
    let mut cfg = config();
    cfg.max_commit_diff_bytes = 1000;
    let (app, _) = app_with(cfg);
    let base = "/browse/github.com/alice/repos/fixture/html";
    for path in [
        format!("{base}/file/big.txt.html"),
        format!("{base}/raw/big.txt"),
    ] {
        let r = get(&app, &path).await;
        assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{path}");
        assert!(r.text().contains("file too large ("), "{}", r.text());
    }
    let r = get(&app, &format!("{base}/file/README.md.html")).await;
    assert_html(&r);
    let r = get(&app, &format!("{base}/raw/README.md")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
}

#[tokio::test]
async fn log_of_a_ref() {
    let (app, _) = app();
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/log.html?ref=feature",
    )
    .await;
    assert_html(&r);
    let tip = git(&["rev-parse", "feature"]);
    assert!(r.text().contains(&format!("commit/{}.html", tip.trim())));
    assert!(!r.text().contains("Merge branch"));
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/log.html?ref=v0.1",
    )
    .await;
    assert_html(&r);
    let refs = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/refs.html",
    )
    .await;
    assert!(refs
        .text()
        .contains("<a href=\"log.html?ref=feature\">feature</a>"));
}

#[tokio::test]
async fn etag_and_conditional_requests() {
    let (app, state) = app();
    let path = "/browse/github.com/alice/repos/fixture/html/files.html";
    let r = get(&app, path).await;
    assert_html(&r);
    let etag = r.header("etag").unwrap().to_string();
    assert!(etag.starts_with('"') && etag.ends_with('"'));
    assert_eq!(
        r.header("cache-control"),
        Some("public, max-age=60, must-revalidate")
    );
    let r = request(&app, "GET", path, &[("if-none-match", &etag)]).await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    assert!(r.body.is_empty());
    let r = request(
        &app,
        "GET",
        path,
        &[("if-none-match", &format!("\"other\", W/{etag}"))],
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    // second full GET is a cache hit with a content length
    let before = state
        .metrics
        .cache_hits
        .load(std::sync::atomic::Ordering::Relaxed);
    let r = get(&app, path).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.header("content-length").is_some());
    assert_eq!(
        state
            .metrics
            .cache_hits
            .load(std::sync::atomic::Ordering::Relaxed),
        before + 1
    );
    // HEAD: headers, no body
    let r = request(&app, "HEAD", path, &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("etag"), Some(etag.as_str()));
    assert!(r.body.is_empty());
    // commit pages are long-lived and carry Last-Modified
    let id = commit_by_subject("Add main.c");
    let r = get(
        &app,
        &format!("/browse/github.com/alice/repos/fixture/html/commit/{id}.html"),
    )
    .await;
    assert_eq!(r.header("cache-control"), Some("public, max-age=86400"));
    assert!(r.header("last-modified").unwrap().ends_with(" GMT"));
}

#[tokio::test]
async fn commit_pages() {
    let (app, _) = app();
    let base = "/browse/github.com/alice/repos/fixture/html/commit";
    // merge: only the first parent, diff against it
    let id = commit_by_subject("Merge branch 'feature'");
    let r = get(&app, &format!("{base}/{id}.html")).await;
    assert_html(&r);
    let t = r.text();
    assert_eq!(t.matches("<b>parent</b>").count(), 1);
    let first_parent = git(&["rev-parse", &format!("{id}^1")]);
    assert!(t.contains(&format!(
        "<b>parent</b> <a href=\"../commit/{0}.html\">{0}</a>",
        first_parent.trim()
    )));
    assert!(t.contains("<td class=\"A\">A</td><td><a href=\"#h0\">feature.txt</a>"));
    assert!(t.contains("1 file changed, 2 insertions(+), 0 deletions(-)"));
    assert!(t.contains(
        "<title>Merge branch &#39;feature&#39; - fixture - Fixture repository &amp; more</title>"
    ));
    // rename
    let id = commit_by_subject("Rename util.c to math.c");
    let t = get(&app, &format!("{base}/{id}.html")).await.text();
    assert!(t.contains("<td class=\"R\">R</td><td><a href=\"#h0\">src/util.c -&gt; src/math.c</a></td><td> | </td><td class=\"num\">0</td>"), "{t}");
    assert!(t.contains("diff --git a/<a id=\"h0\" href=\"../file/src/util.c.html\">src/util.c</a> b/<a href=\"../file/src/math.c.html\">src/math.c</a>"));
    // copy of a modified file
    let id = commit_by_subject("Modify main.c and copy it");
    let t = get(&app, &format!("{base}/{id}.html")).await.text();
    assert!(
        t.contains("<td class=\"M\">M</td><td><a href=\"#h0\">src/main.c</a>"),
        "{t}"
    );
    assert!(
        t.contains(
            "<td class=\"C\">C</td><td><a href=\"#h1\">src/main.c -&gt; src/main_copy.c</a>"
        ),
        "{t}"
    );
    // copy of an unmodified file is an addition
    let id = commit_by_subject("Copy LICENSE into docs");
    let t = get(&app, &format!("{base}/{id}.html")).await.text();
    assert!(
        t.contains("<td class=\"A\">A</td><td><a href=\"#h0\">docs/LICENSE</a>"),
        "{t}"
    );
    // typechange
    let id = commit_by_subject("Turn docs/LICENSE into a symlink");
    let t = get(&app, &format!("{base}/{id}.html")).await.text();
    assert!(
        t.contains("<td class=\"T\">T</td><td><a href=\"#h0\">docs/LICENSE</a>"),
        "{t}"
    );
    // binary
    let id = commit_by_subject("Modify binary file");
    let t = get(&app, &format!("{base}/{id}.html")).await.text();
    assert!(t.contains("Binary files differ."), "{t}");
    assert!(
        t.contains(
            "<td class=\"num\">0</td><td><span class=\"i\"></span><span class=\"d\"></span></td>"
        ),
        "{t}"
    );
    // no newline marker is rendered like stagit does
    let id = commit_by_subject("Add trailing newline");
    let t = get(&app, &format!("{base}/{id}.html")).await.text();
    assert!(
        t.contains("class=\"d\">-\\ No newline at end of file\n</a>"),
        "{t}"
    );
    assert!(
        t.contains("1 file changed, 1 insertion(+), 2 deletions(-)"),
        "{t}"
    );
    // root commit diffs against the empty tree
    let id = commit_by_subject("Initial commit");
    let t = get(&app, &format!("{base}/{id}.html")).await.text();
    assert!(!t.contains("<b>parent</b>"));
    assert!(t.contains("<td class=\"A\">A</td><td><a href=\"#h0\">LICENSE</a>"));
    assert!(t.contains("<td class=\"A\">A</td><td><a href=\"#h1\">README.md</a>"));
    assert!(t.contains("@@ -0,0 +1,3 @@"));
    // empty message, author with quote, negative offset
    let id = git(&[
        "log",
        "--all",
        "--format=%H",
        "-n1",
        "--grep=^$",
        "--author=Bob O",
    ]);
    let t = get(&app, &format!("{base}/{}.html", id.trim()))
        .await
        .text();
    assert!(t.contains("<b>Author:</b> Bob O&#39;Brien &lt;<a href=\"mailto:bob@example.com\">bob@example.com</a>&gt;"), "{t}");
    assert!(t.contains(" -0500\n"), "{t}");
    // hunk headers carry function context and anchors
    let id = commit_by_subject("Edit big.txt in several places");
    let t = get(&app, &format!("{base}/{id}.html")).await.text();
    assert!(
        t.contains("<a href=\"#h0-0\" id=\"h0-0\" class=\"h\">@@ -97,12 +97,12 @@ line 96\n</a>"),
        "{t}"
    );
    assert!(
        t.contains("class=\"h\">@@ -4997,7 +4997,7 @@ line 4996\n</a>"),
        "{t}"
    );
    assert!(t.contains("<a href=\"#h0-0-3\" id=\"h0-0-3\" class=\"d\">-line 100\n</a><a href=\"#h0-0-4\" id=\"h0-0-4\" class=\"i\">+def line_100():\n</a>"), "{t}");
}

#[tokio::test]
async fn files_page() {
    let (app, _) = app();
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/files.html",
    )
    .await;
    assert_html(&r);
    let t = r.text();
    assert!(t.contains("<tr><td>-rw-r--r--</td><td><a href=\"file/README.md.html\">README.md</a></td><td class=\"num\" align=\"right\">43B</td></tr>"), "{t}");
    assert!(
        t.contains("<tr><td>-rwxr-xr-x</td><td><a href=\"file/run.sh.html\">run.sh</a>"),
        "{t}"
    );
    assert!(t.contains("<tr><td>l---------</td><td><a href=\"file/docs/LICENSE.html\">docs/LICENSE</a></td><td class=\"num\" align=\"right\">10B</td></tr>"), "{t}");
    assert!(t.contains("<tr><td>m---------</td><td><a href=\"file/.gitmodules.html\">vendor/lib</a> @ 1234567</td><td class=\"num\" align=\"right\"></td></tr>"), "{t}");
    assert!(
        t.contains("<a href=\"file/odd%20dir/a%23b.txt.html\">odd dir/a#b.txt</a>"),
        "{t}"
    );
    assert!(
        t.contains("<a href=\"file/odd%20dir/100%25.txt.html\">odd dir/100%.txt</a>"),
        "{t}"
    );
    assert!(
        t.contains("<a href=\"file/odd%20dir/c%2Bd.txt.html\">odd dir/c+d.txt</a>"),
        "{t}"
    );
    assert!(
        t.contains("<a href=\"file/odd%20dir/%C3%BCmlaut.txt.html\">odd dir/ümlaut.txt</a>"),
        "{t}"
    );
    assert!(
        t.contains("<a href=\"file/odd%20dir/it%27s.txt.html\">odd dir/it&#39;s.txt</a>"),
        "{t}"
    );
    assert!(
        t.contains(
            "<a href=\"file/odd%20dir/x%3Cy%3E%26z.txt.html\">odd dir/x&lt;y&gt;&amp;z.txt</a>"
        ),
        "{t}"
    );
    // tree order: ".ci" style dot files first, directories interleaved by name
    let pos = |s: &str| t.find(s).unwrap_or_else(|| panic!("{s} missing"));
    assert!(pos("file/.gitmodules.html") < pos("file/LICENSE.html"));
    assert!(pos("file/README.md.html") < pos("file/big.txt.html"));
    // the header links README / LICENSE / Submodules
    assert!(t.contains("<a href=\"file/.gitmodules.html\">Submodules</a> | <a href=\"file/README.md.html\">README</a> | <a href=\"file/LICENSE.html\">LICENSE</a>"));
    // line counts mode
    let mut cfg = config();
    cfg.files_line_counts = true;
    let (app, _) = app_with(cfg);
    let t = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/files.html",
    )
    .await
    .text();
    assert!(t.contains("<a href=\"file/README.md.html\">README.md</a></td><td class=\"num\" align=\"right\">3L</td>"), "{t}");
    assert!(t.contains("<a href=\"file/empty.txt.html\">empty.txt</a></td><td class=\"num\" align=\"right\">0B</td>"), "{t}");
    assert!(t.contains("<a href=\"file/image.png.html\">image.png</a></td><td class=\"num\" align=\"right\">22B</td>"), "{t}");
    // max-files cap
    let mut cfg = config();
    cfg.max_files = 3;
    let (app, _) = app_with(cfg);
    let t = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/files.html",
    )
    .await
    .text();
    assert_eq!(
        t.matches("<tr><td>-").count()
            + t.matches("<tr><td>l").count()
            + t.matches("<tr><td>m").count(),
        3
    );
    assert!(t.contains("more files not shown"));
}

#[tokio::test]
async fn blob_pages() {
    let (app, _) = app();
    let base = "/browse/github.com/alice/repos/fixture/html/file";
    let r = get(&app, &format!("{base}/README.md.html")).await;
    assert_html(&r);
    let t = r.text();
    assert!(t.contains("<title>README.md - fixture - Fixture repository &amp; more</title>"));
    assert!(t.contains("<p> README.md (43B)</p><hr/><pre id=\"blob\">\n<a href=\"#l1\" class=\"line\" id=\"l1\">      1</a> # Fixture\n<a href=\"#l2\" class=\"line\" id=\"l2\">      2</a> \n<a href=\"#l3\" class=\"line\" id=\"l3\">      3</a> A &lt;test&gt; repository &amp; &quot;quotes&quot;.\n</pre>\n"), "{t}");
    // nested path: relpath has one ../ per level
    let t = get(&app, &format!("{base}/src/main.c.html")).await.text();
    assert!(t.contains("href=\"../../style.css\""));
    assert!(t.contains("<a href=\"../../log.html\">Log</a>"));
    let t = get(&app, &format!("{base}/odd%20dir/a%23b.txt.html"))
        .await
        .text();
    assert!(t.contains("<p> a#b.txt (5B)</p>"), "{t}");
    // CRLF: \r dropped, no newline at end still numbered
    let t = get(&app, &format!("{base}/crlf.txt.html")).await.text();
    assert!(t.contains("id=\"l3\">      3</a> c\n</pre>"), "{t}");
    let t = get(&app, &format!("{base}/README.link.html")).await.text();
    assert!(t.contains("id=\"l1\">      1</a> README.md</pre>"), "{t}");
    assert!(!t.contains('\r'));
    // empty file
    let t = get(&app, &format!("{base}/empty.txt.html")).await.text();
    assert!(
        t.contains("<p> empty.txt (0B)</p><hr/><pre id=\"blob\">\n</pre>\n"),
        "{t}"
    );
    // binary
    let t = get(&app, &format!("{base}/image.png.html")).await.text();
    assert!(
        t.contains("<p> image.png (22B)</p><hr/><p>Binary file.</p>\n</div>"),
        "{t}"
    );
    // symlink shows its target
    let t = get(&app, &format!("{base}/docs/LICENSE.html")).await.text();
    assert!(t.contains("id=\"l1\">      1</a> ../LICENSE</pre>"), "{t}");
    // non-UTF-8 content is passed through
    let r = get(&app, &format!("{base}/latin1.txt.html")).await;
    assert!(r.body.windows(4).any(|w| w == b"caf\xe9"));
    // a file literally named x.html would be x.html.html; big file
    let t = get(&app, &format!("{base}/big.txt.html")).await.text();
    assert!(t.contains("id=\"l9999\">   9999</a> line 9999\n"), "{t}");
    // truncation
    let mut cfg = config();
    cfg.max_blob_bytes = 1000;
    let (app, _) = app_with(cfg);
    let t = get(&app, &format!("{base}/big.txt.html")).await.text();
    assert!(t.contains("</pre>\n<p>File truncated, "), "{t}");
    assert!(t.contains("<a href=\"../raw/big.txt\">Raw file</a>"), "{t}");
    assert!(!t.contains("id=\"l200\""));
    // raw
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/raw/README.md",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
    assert_eq!(r.body, b"# Fixture\n\nA <test> repository & \"quotes\".\n");
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/raw/image.png",
    )
    .await;
    assert_eq!(r.header("content-type"), Some("application/octet-stream"));
}

#[tokio::test]
async fn refs_and_feeds() {
    let (app, _) = app();
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/refs.html",
    )
    .await;
    assert_html(&r);
    let t = r.text();
    assert!(t.contains("<h2>Branches</h2><table id=\"branches\">"));
    assert!(t.contains("<h2>Tags</h2><table id=\"tags\">"));
    for name in ["main", "feature", "stable", "v1.0", "v0.1", "v0.1-light"] {
        assert!(
            t.contains(&format!(">{name}</a></td>")),
            "{name} missing in {t}"
        );
    }
    assert!(!t.contains("tree-tag"));
    assert!(!t.contains("old-tag"));
    assert!(!t.contains("gitbackup"));
    // ordering: main (newest) before stable before feature
    let p = |s: &str| t.find(&format!(">{s}</a>")).unwrap();
    assert!(p("main") < p("stable") && p("stable") < p("feature"));
    assert!(p("v1.0") < p("v0.1-light") && p("v0.1-light") < p("v0.1"));
    // without links
    let mut cfg = config();
    cfg.link_refs = false;
    let (app2, _) = app_with(cfg);
    let t = get(
        &app2,
        "/browse/github.com/alice/repos/fixture/html/refs.html",
    )
    .await
    .text();
    assert!(t.contains("<tr><td>main</td><td>"));

    let r = get(&app, "/browse/github.com/alice/repos/fixture/html/atom.xml").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.header("content-type"),
        Some("application/atom+xml; charset=utf-8")
    );
    let t = r.text();
    assert!(t.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n<title>fixture, branch HEAD</title>\n<subtitle>Fixture repository &amp; more</subtitle>\n<entry>\n"));
    assert!(t.ends_with("</entry>\n</feed>\n"));
    assert!(t.contains("<link rel=\"alternate\" type=\"text/html\" href=\"commit/"));
    let n = git(&["rev-list", "--count", "main"])
        .trim()
        .parse::<usize>()
        .unwrap();
    assert_eq!(t.matches("<entry>").count(), n.min(100));
    let r = get(&app, "/browse/github.com/alice/repos/fixture/html/tags.xml").await;
    let t = r.text();
    assert!(
        t.contains("<title>[v1.0] Edit big.txt in several places</title>"),
        "{t}"
    );
    assert_eq!(t.matches("<entry>").count(), 3);
    // absolute feed links with --public-url
    let mut cfg = config();
    cfg.public_url = Some("https://example.org/browse/".into());
    let (app, _) = app_with(cfg);
    let t = get(&app, "/browse/github.com/alice/repos/fixture/html/atom.xml")
        .await
        .text();
    assert!(
        t.contains("href=\"https://example.org/browse/github.com/alice/repos/fixture/html/commit/"),
        "{t}"
    );
}

#[tokio::test]
async fn versions_and_forks_see_their_own_history() {
    let (app, _) = app();
    let t = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html.v1/log.html",
    )
    .await
    .text();
    assert!(!t.contains("Edit big.txt"));
    assert!(t.contains("Add non-UTF-8 bytes"), "{t}");
    let t = get(
        &app,
        "/browse/github.com/alice/repos/fixture/forks/bob/html/log.html",
    )
    .await
    .text();
    assert!(t.contains("Bob&#39;s fork commit"));
    assert!(t.contains("Edit big.txt"));
    let t = get(
        &app,
        "/browse/github.com/alice/repos/fixture/forks/bob/html.v1/files.html",
    )
    .await
    .text();
    assert!(t.contains("BOB.txt"));
    let t = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html.v1/refs.html",
    )
    .await
    .text();
    assert!(t.contains(">v0.1</a>") && !t.contains("v1.0"));
}

#[tokio::test]
async fn empty_repository() {
    let (app, _) = app();
    for page in ["log.html", "files.html", "refs.html", "atom.xml"] {
        let r = get(
            &app,
            &format!("/browse/github.com/alice/repos/empty/html/{page}"),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{page}");
    }
    let t = get(&app, "/browse/github.com/alice/repos/empty/html/log.html")
        .await
        .text();
    assert!(t.contains("empty repository"));
    assert!(!t.contains("git clone"));
    let r = get(
        &app,
        "/browse/github.com/alice/repos/empty/html/file/x.html",
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn overload_returns_503() {
    let mut cfg = config();
    cfg.render_threads = Some(1);
    cfg.max_queue = 0;
    cfg.queue_timeout = std::time::Duration::from_millis(50);
    let (app, state) = app_with(cfg);
    let _held = state.permits.clone().try_acquire_owned().unwrap();
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/refs.html",
    )
    .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.header("retry-after"), Some("5"));
    // with a queue slot the request waits, then times out
    let mut cfg = config();
    cfg.render_threads = Some(1);
    cfg.max_queue = 1;
    cfg.queue_timeout = std::time::Duration::from_millis(50);
    let (app, state) = app_with(cfg);
    let _held = state.permits.clone().try_acquire_owned().unwrap();
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/refs.html",
    )
    .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        state
            .metrics
            .queue_drops
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    // cached pages and 304s bypass the pool entirely
    let (app, state) = app_with(config());
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/refs.html",
    )
    .await;
    let etag = r.header("etag").unwrap().to_string();
    let _held = state.permits.clone().try_acquire_owned().unwrap();
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/refs.html",
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = request(
        &app,
        "GET",
        "/browse/github.com/alice/repos/fixture/html/refs.html",
        &[("if-none-match", &etag)],
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn render_timeout_notice() {
    use streagit::render::out::{RenderError, VecOut};
    let root = fixture_root().join("github.com/alice/repos/fixture/repo.git");
    let repo = gix::open(&root).unwrap();
    let tip = streagit::git::repo::resolve_head(&repo).unwrap();
    let meta = streagit::git::repo::read_meta(
        &streagit::git::repo::RepoRef {
            git_dir: root.clone(),
            slot: "fixture".into(),
            rel: "fixture/repo.git".into(),
        },
        streagit::config::NameFrom::Slot,
    );
    let mut ctx = streagit::http::errors::error_ctx(meta, String::new());
    ctx.page_size = 10;
    ctx.deadline = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let stats = streagit::cache::stats::StatsCache::new(10);
    let mut out = VecOut::default();
    let err = streagit::render::log::render_log(&repo, &ctx, &stats, tip, None, "", &mut out)
        .unwrap_err();
    assert!(matches!(err, RenderError::Timeout));
    // the header was already written when the deadline hit
    assert!(String::from_utf8_lossy(&out.0).contains("<table id=\"log\">"));
}

#[test]
fn disconnected_client_cancels_the_render() {
    use streagit::http::stream::ChunkOut;
    use streagit::render::out::{Out, RenderError};
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut out = ChunkOut::new(tx, 1 << 20, deadline);
    drop(rx);
    let big = vec![b'x'; 70 * 1024];
    assert!(matches!(out.write(&big), Err(RenderError::Cancelled)));
}

#[tokio::test]
async fn large_commit_pages_are_not_cached_but_small_ones_are() {
    let mut cfg = config();
    cfg.cache_max_entry = 8192;
    let (app, state) = app_with(cfg);
    let id = commit_by_subject("Add big file");
    let path = format!("/browse/github.com/alice/repos/fixture/html/commit/{id}.html");
    let r = get(&app, &path).await;
    assert_html(&r);
    assert!(r.body.len() > 8192);
    let (entries, _, _, _) = state.render_cache.stats();
    assert_eq!(entries, 0);
    let r = get(
        &app,
        "/browse/github.com/alice/repos/fixture/html/refs.html",
    )
    .await;
    assert!(r.body.len() < 8192);
    // the render thread stores the page right after sending the last chunk,
    // so the client can see the end of the body before the cache insert
    let mut entries = 0;
    for _ in 0..100 {
        entries = state.render_cache.stats().0;
        if entries > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(entries, 1);
    // the diffstat cache learned the commit's counts
    let oid = gix::ObjectId::from_hex(id.as_bytes()).unwrap();
    assert_eq!(
        state.stats.get_stats(&oid),
        Some(streagit::git::diff::Stats {
            files: 1,
            adds: 10000,
            dels: 0
        })
    );
}

#[tokio::test]
async fn diff_size_limits() {
    let mut cfg = config();
    cfg.max_diff_blob_bytes = 1000;
    let (app, _) = app_with(cfg);
    let id = commit_by_subject("Add big file");
    let t = get(
        &app,
        &format!("/browse/github.com/alice/repos/fixture/html/commit/{id}.html"),
    )
    .await
    .text();
    assert!(t.contains("Diff skipped, file too large ("), "{t}");
    assert!(t.contains("<td class=\"A\">A</td><td><a href=\"#h0\">big.txt</a></td><td> | </td><td class=\"num\">0</td>"), "{t}");
}
