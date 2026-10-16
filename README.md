# streagit

**streagit** is an on-demand, streaming, [stagit](https://codemadness.org/stagit.html)-compatible
git web renderer written in Rust. It serves the pages stagit would pre-render
(`log.html`, `files.html`, `refs.html`, `commit/<id>.html`, `file/<path>.html`,
`atom.xml`, `tags.xml`) straight from bare repositories, at request time, over
HTTP. Nothing is written to disk; results are cached in memory keyed by immutable
git object ids.

It exists so that [gitbackup](https://github.com/MarvAmBass/gitbackup) can stop
running `stagit` over hundreds of mirrored repositories after every change: with
stagit, a ComfyUI-sized mirror costs ~10 s of CPU and 162 MB of HTML per
rewrite, almost all of it for diffs of pages nobody opens. streagit does the work
for the page that is requested, once.

Single static binary, no runtime dependencies, pure Rust (`gix` for git access,
`axum`/`tokio` for HTTP). The HTML is byte-compatible with stagit 1.3 (verified
by a golden test against real stagit output), so existing stylesheets and links
keep working.

## Usage

```
streagit --root /data --listen 127.0.0.1:8081 --prefix /browse
```

Repositories are bare git directories anywhere below `--root`, laid out the
way gitbackup stores them (a repository folder is called a *slot*):

```
<root>/github.com/<owner>/repos/<name>/repo.git        -> <prefix>/github.com/<owner>/repos/<name>/html/log.html
<root>/github.com/<owner>/repos/<name>/repo.v2.git     -> .../html.v2/log.html
<root>/github.com/<owner>/repos/<name>/wiki.git        -> .../html.wiki/log.html
<root>/github.com/<owner>/repos/<name>/wiki.v1.git     -> .../html.wiki.v1/log.html
<root>/github.com/<owner>/repos/<name>/forks/x/repo.git -> .../forks/x/html/log.html
any <dir>.git below the root                          -> <prefix>/<dir>.git/log.html  (--allow-git-paths)
```

Each repository may contain the plain-text files `description`, `url` and
`owner` (first line used). `objects/info/alternates` with relative paths,
packed refs and symlinked slots inside the root are supported; refs outside
`refs/heads/` and `refs/tags/` (gitbackup's `refs/gitbackup/versions/...`)
are ignored.

### Pages

| URL (under the repo prefix) | content |
|---|---|
| `log.html` | commit log of HEAD with files/+/- per commit, `--page-size` rows, `?from=<id>` for the next page, `?ref=<branch or tag>` for another ref |
| `commit/<id>.html` | commit header, diffstat, diff (first parent for merges) |
| `files.html` | every file in HEAD (`--files-line-counts` shows line counts like stagit) |
| `file/<path>.html` | blob with line numbers (truncated beyond `--max-blob-bytes`) |
| `raw/<path>` | raw blob |
| `refs.html` | branches and tags |
| `atom.xml`, `tags.xml` | feeds (`--public-url` makes the links absolute) |
| `style.css`, `logo.png`, `favicon.png` | embedded stagit assets, or from `--assets-dir` |
| `<prefix>/healthz`, `<prefix>/metrics`, `<prefix>/robots.txt` | health, Prometheus metrics, robots policy (`--robots allow`) |

### Options

Every flag can also be set through `STREAGIT_<NAME>` (e.g. `STREAGIT_ROOT`).

| flag | default | meaning |
|---|---|---|
| `--listen` | `127.0.0.1:8081` | bind address |
| `--root` | required | data root |
| `--prefix` | `/` | URL prefix stripped before resolution |
| `--public-url` | – | absolute URL of the prefix, for feed links |
| `--page-size` | 100 | log rows per page (max 500) |
| `--render-threads` | cores/4 (min 1) | concurrent renders |
| `--max-queue` / `--queue-timeout` | 32 / 10s | requests waiting for a render slot; beyond that 503 + `Retry-After` |
| `--render-timeout` | 30s | per render (log pages: 2x); the page ends with a notice when exceeded |
| `--cache-bytes` / `--cache-max-entry` | 64MiB / 1MiB | in-memory render cache |
| `--stats-entries` | 2000000 | diffstat cache entries (commit id -> files/+/-) |
| `--cache-dir` | – | persist the diffstat cache across restarts |
| `--open-repos` / `--object-cache-bytes` | 64 / 8MiB | repository handles kept open, gix object cache per render |
| `--max-blob-bytes` | 2MiB | blob page truncation |
| `--max-diff-blob-bytes` / `--max-commit-diff-bytes` | 4MiB / 64MiB | diffs skipped above these sizes |
| `--max-files` | 20000 | rows on files.html |
| `--files-line-counts` | off | `NL` sizes on files.html like stagit (reads every blob) |
| `--feed-entries` | 100 | entries in atom.xml / tags.xml |
| `--name-from` | `slot` | repository name shown: `slot`, `dir` (stagit's `repo.git` → `repo`), `owner-slot` |
| `--assets-dir` | embedded | directory with `style.css`, `logo.png`, `favicon.png` |
| `--allow-git-paths` | true | direct `<name>.git/...` URLs |
| `--link-refs` | true | link ref names on refs.html to `log.html?ref=` |
| `--robots` | `disallow` | robots.txt policy |
| `--nice` | 0 | process niceness |
| `--compression` | true | gzip responses |
| `--max-connections` | 256 | concurrent connections |
| `--log-level` | `info` | `RUST_LOG`-style filter |

### Caching and limits

- Every page carries a strong `ETag` derived from the object ids it depends on
  (commit id, tree id, tip + page, hash of the refs); `If-None-Match` answers
  `304` before anything is rendered.
- Rendered pages up to `--cache-max-entry` are kept in a bytes-bounded LRU.
  Concurrent requests for the same page share one render.
- The files/+/- columns of the log need a diff per commit; the result is kept
  per commit id (and optionally persisted with `--cache-dir`), so a log page
  costs its diffs once.
- Renders run in a bounded pool; cache hits and 304s never touch it. Pages are
  streamed in 64 KiB chunks and a render stops as soon as the client goes away.
- stagit's "Diff is too large, output suppressed." thresholds apply (1000 files
  or 100k lines); additionally single blobs above `--max-diff-blob-bytes` and
  commits above `--max-commit-diff-bytes` are listed but not diffed.

### How gitbackup uses it

gitbackup starts `streagit --listen 127.0.0.1:8081 --root <data> --prefix
/browse --public-url $GITBACKUP_BROWSE_URL --cache-dir <state>/streagit` next
to its own web server and reverse-proxies `/browse/.../html*/...` (and
`.git/...` paths) to it; everything else under `/browse/` stays a file
server. The landing pages link `html/log.html`, `html.v1/log.html`,
`html.wiki/log.html` and so on as before. See `SPEC.md` §13.3.

## Building

No toolchain on the host is needed:

```sh
# static musl binary (native architecture of the builder)
docker run --rm -v "$PWD:/src" -w /src rust:alpine sh -c \
  'apk add musl-dev && cargo build --release --locked'
# -> target/release/streagit

# container image for the NAS (x86_64)
docker build --platform linux/amd64 -t streagit .
docker run --rm -p 8081:8081 -v /path/to/data:/data:ro streagit \
  --listen 0.0.0.0:8081 --root /data --prefix /browse
```

`rust:alpine` targets musl natively, so the binary is fully static and runs on
any `alpine:3` (or `scratch`) image of the same architecture.

## Testing

```sh
docker run --rm -v "$PWD:/src" -w /src rust:alpine sh -c \
  'apk add musl-dev git && cargo test'
```

The integration tests build a gitbackup-style fixture with the `git` CLI
(`tests/fixtures/make_fixture.sh`: merges, exact renames and copies, a
typechange, binary files, a submodule entry, odd and non-UTF-8 paths, a frozen
version and a nested fork borrowing objects via relative alternates, preserved
`refs/gitbackup/versions/...` refs, a symlinked star view and a symlink that
escapes the root). `tests/golden/` holds stagit 1.3's output for that fixture;
`golden_test.rs` renders every page with streagit and compares byte for byte
(regenerate with `tests/golden/gen.sh`, which builds stagit in `alpine:3`).

`scripts/bench.sh <repo.git>` compares per-page latency against a full
stagit render on a real repository.

## Benchmark

ComfyUI mirror (3,506 commits, 432 files in HEAD, 58 tags, 80 MB packs),
both sides in `rust:alpine` on Docker Desktop (Apple Silicon host, arm64
container), repository on the container file system:

| | time |
|---|---|
| stagit 1.3: full render, all 3,506 commit pages + 432 file pages (162 MB) | **8.0 s** |
| streagit: `log.html` (100 rows, 100 diffs, cold) | 49 ms |
| streagit: `log.html` (cached) | < 1 ms |
| streagit: `log.html` with `If-None-Match` (304) | < 1 ms |
| streagit: largest commit page (79 files, 400k lines changed), cold | 106 ms (25 MB page; 0.41 s gzipped to 4.1 MB) |
| streagit: `files.html` cold | 1 ms |
| streagit: `refs.html` cold | 1 ms |
| streagit: whole history through log pages (every diffstat once) | 2.0 s (35 pages) |

Peak RSS during the whole-history walk was 185 MB (render cache 64 MiB,
four render threads with 8 MiB object caches, diffstat cache). Run
`scripts/bench.sh` to reproduce.

## Fidelity and deviations from stagit

The page layout, ids and classes are stagit's; the golden test pins them. On
purpose streagit differs in a few places (all listed in `SPEC.md`):

- the description is trimmed (stagit leaks its trailing newline into
  `<title>`); the repository name defaults to the slot name instead of `repo`;
  the Atom feed title uses that name instead of `repo.git`;
- `log.html` is paginated (`?from=`), `refs.html` names link to `log.html?ref=`;
- `files.html` shows byte sizes unless `--files-line-counts` is on;
- a tag that points at a tree is skipped instead of truncating the tag list;
- blobs and diffs have size limits; diffs use imara-diff's Myers, which can
  pick different but equivalent hunks for ambiguous inputs (counts match).

## License

MIT, see `LICENSE`. The page layout, `assets/style.css`, `assets/logo.png` and
`assets/favicon.png` are derived from stagit, MIT/X Consortium License,
(c) 2015-2026 Hiltjo Posthuma; its license text is included in `LICENSE`. No
stagit C code is used; the output format is re-implemented.
