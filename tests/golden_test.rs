//! Golden comparison with stagit 1.3's output for the fixture (SPEC §12.2).
//!
//! `tests/golden/fixture/` holds what `stagit` wrote for the main fixture repo
//! (regenerate with `tests/golden/gen.sh`). streagit renders the same pages
//! and must match byte for byte after a few documented normalisations:
//!
//! - stagit keeps the trailing newline of the `description` file (it leaks into
//!   `<title>`, `<span class="desc">` and `<subtitle>`); streagit trims it.
//! - stagit names the atom feed after the directory (`repo.git Atom Feed`);
//!   streagit uses the display name (`repo`).
//! - the golden run has line counts on files.html; the test enables
//!   `--files-line-counts`, uses `--name-from dir` and `--link-refs false`.

mod common;

use std::path::Path;

use common::*;

fn golden_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/fixture")
}

fn normalise(golden: &[u8]) -> Vec<u8> {
    let s = String::from_utf8_lossy(golden).into_owned();
    let s = s
        .replace("\n</title>", "</title>")
        .replace("\n</span></td></tr>", "</span></td></tr>")
        .replace("\n</subtitle>", "</subtitle>")
        .replace("repo.git Atom Feed", "repo Atom Feed");
    // lossy conversion would corrupt non-UTF-8 bytes; only use the string form when
    // the input was valid UTF-8, otherwise do byte-level replacement
    if std::str::from_utf8(golden).is_ok() {
        s.into_bytes()
    } else {
        let mut v = golden.to_vec();
        for (from, to) in [
            (&b"\n</title>"[..], &b"</title>"[..]),
            (b"\n</span></td></tr>", b"</span></td></tr>"),
            (b"\n</subtitle>", b"</subtitle>"),
            (b"repo.git Atom Feed", b"repo Atom Feed"),
        ] {
            v = replace_bytes(&v, from, to);
        }
        v
    }
}

fn replace_bytes(hay: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

fn first_diff(a: &[u8], b: &[u8]) -> String {
    let i = a
        .iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let ctx = |v: &[u8]| {
        String::from_utf8_lossy(&v[i.saturating_sub(120)..(i + 200).min(v.len())]).into_owned()
    };
    format!(
        "first difference at byte {i}\n--- stagit:\n{}\n--- streagit:\n{}\n",
        ctx(a),
        ctx(b)
    )
}

/// Skip when the golden set was generated for a different fixture (e.g. a host
/// filesystem that refused the non-UTF-8 file name changes every id).
fn golden_matches_fixture() -> bool {
    let head = git(&["rev-parse", "HEAD"]);
    match std::fs::read(golden_dir().join("log.html")) {
        Ok(log) => String::from_utf8_lossy(&log).contains(head.trim()),
        Err(_) => false,
    }
}

fn golden_config() -> streagit::config::Config {
    let mut cfg = config();
    cfg.page_size = 500;
    cfg.files_line_counts = true;
    cfg.name_from = streagit::config::NameFrom::Dir;
    cfg.link_refs = false;
    cfg
}

async fn compare(app: &axum::Router, page: &str) {
    let golden = std::fs::read(golden_dir().join(page)).unwrap_or_else(|e| panic!("{page}: {e}"));
    let expected = normalise(&golden);
    let url: String = page
        .split('/')
        .map(|seg| {
            let mut v = streagit::render::out::VecOut::default();
            streagit::render::out::Out::pct(&mut v, seg.as_bytes()).unwrap();
            String::from_utf8(v.0).unwrap()
        })
        .collect::<Vec<_>>()
        .join("/");
    let r = get(app, &format!("/browse{SLOT}/html/{url}")).await;
    assert_eq!(r.status, 200, "{page}: {}", r.text());
    assert!(
        r.body == expected,
        "{page}: {}",
        first_diff(&expected, &r.body)
    );
}

#[tokio::test]
async fn top_level_pages_match_stagit() {
    if !golden_matches_fixture() {
        eprintln!("golden set does not match this fixture build; skipping");
        return;
    }
    let (app, _) = app_with(golden_config());
    for page in [
        "log.html",
        "files.html",
        "refs.html",
        "atom.xml",
        "tags.xml",
    ] {
        compare(&app, page).await;
    }
}

#[tokio::test]
async fn every_commit_page_matches_stagit() {
    if !golden_matches_fixture() {
        eprintln!("golden set does not match this fixture build; skipping");
        return;
    }
    let (app, _) = app_with(golden_config());
    let mut n = 0;
    for entry in std::fs::read_dir(golden_dir().join("commit")).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        compare(&app, &format!("commit/{name}")).await;
        n += 1;
    }
    assert!(
        n >= 30,
        "expected at least 30 golden commit pages, found {n}"
    );
}

#[tokio::test]
async fn file_pages_match_stagit() {
    if !golden_matches_fixture() {
        eprintln!("golden set does not match this fixture build; skipping");
        return;
    }
    let (app, _) = app_with(golden_config());
    // The page list comes from the git tree, not from a directory walk: a
    // checkout on macOS/APFS (or one synced by tools that normalise file names)
    // may hold `ümlaut.txt.html` in NFD form, which no longer matches the NFC
    // name inside the repository. Opening the golden file by its NFC name works
    // on every file system that stores either form.
    let mut pages = Vec::new();
    let listing = git_bytes(&["ls-tree", "-r", "--name-only", "-z", "HEAD"]);
    for path in listing.split(|&c| c == 0) {
        // the non-UTF-8 name (`caf\xe9.txt`) has no golden page (gen.sh drops it)
        let Ok(path) = std::str::from_utf8(path) else {
            continue;
        };
        if path.is_empty() || path == "vendor/lib" {
            continue;
        }
        let page = format!("file/{path}.html");
        if golden_dir().join(&page).is_file() {
            pages.push(page);
        } else {
            // gen.sh also drops the 10k-line page
            assert_eq!(path, "big.txt", "golden page missing for {path:?}");
        }
    }
    assert!(pages.len() >= 20, "{pages:?}");
    for page in pages {
        compare(&app, &page).await;
    }
}
