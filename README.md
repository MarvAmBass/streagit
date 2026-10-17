# streagit

**streagit** is a small HTTP server that renders
[stagit](https://codemadness.org/stagit.html)-compatible web pages for a
directory of bare git repositories – on demand and streaming, instead of
pre-rendering them to static files. It serves the pages stagit would write
(`log.html`, `files.html`, `refs.html`, `commit/<id>.html`, `file/<path>.html`,
`atom.xml`, `tags.xml`) straight from the repositories at request time. Nothing
is written to disk; results are cached in memory keyed by immutable git object
ids.

streagit is a re-implementation of the output of
[stagit](https://codemadness.org/stagit.html) by Hiltjo Posthuma. It is not
affiliated with or endorsed by stagit or codemadness.

stagit is designed to pre-render (mostly append-only) repositories as static
files and is efficient at that; its `-c` cache makes fast-forward updates
cheap. streagit targets a different case: many large mirrors that are rarely
viewed and whose history may be rewritten, where re-rendering every commit
page after each change costs far more than rendering the few pages someone
actually opens. Rendering one page on demand is typically two to three
orders of magnitude cheaper than a full stagit render of the same repository
(see [Benchmark](#benchmark)).

Single static binary, no runtime dependencies, pure Rust (`gix` for git access,
`axum`/`tokio` for HTTP). The HTML is byte-compatible with stagit 1.3 (verified
by a golden test against real stagit output), so existing stylesheets and links
keep working.

**Status:** v0.1 – early, but tested against stagit's output and real
repositories. SHA-256 repositories are untested; HTTP `Range` requests are not
supported; there is no authentication (see [Security](#security)).

## Quick start

Given a directory of bare repositories, e.g. `~/git/myproject.git`
(`git clone --mirror …` or `git init --bare`):

```sh
streagit --root ~/git
# open http://127.0.0.1:8081/myproject.git/log.html
```

or with Docker (build the image first, see [Install](#install)):

```sh
docker run --rm -p 127.0.0.1:8081:8081 -v ~/git:/data:ro streagit \
  --listen 0.0.0.0:8081 --root /data
# open http://127.0.0.1:8081/myproject.git/log.html
```

Every `<dir>.git` below the root is reachable as `<prefix>/<path>/<dir>.git/`
(e.g. `~/git/team/tool.git` → `/team/tool.git/log.html`). With such a plain
layout, `--name-from dir` names each repository after its directory. The
optional files `description`, `url` and `owner` inside a repository are shown
in the page header like stagit does.

## Install

There are no prebuilt binaries or images; build from source:

```sh
cargo install --git https://github.com/MarvAmBass/streagit --tag v0.1.3
```

or build the static binary or the Docker image without a local Rust
toolchain, see [Building](#building).

## Usage

```
streagit --root /data --listen 127.0.0.1:8081 --prefix /browse
```

Repositories are bare git directories anywhere below `--root`. Besides plain
`<name>.git` directories, streagit understands the layout of the mirror tool
gitbackup (see [Integration example](#integration-example-gitbackup)), where a
repository folder is called a *slot*:

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
`refs/heads/` and `refs/tags/` (e.g. gitbackup's `refs/gitbackup/versions/...`)
are ignored.

### Pages

| URL (under the repo prefix) | content |
|---|---|
| `log.html` | commit log of HEAD with files/+/- per commit, `--page-size` rows, `?from=<id>` for the next page (the walk restarts at the tip and skips to `<id>`, so pages stay continuous across merges), `?ref=<branch or tag>` for another ref |
| `commit/<id>.html` | commit header, diffstat, diff (first parent for merges) |
| `files.html` | every file in HEAD (`--files-line-counts` shows line counts like stagit) |
| `file/<path>.html` | blob with line numbers (truncated beyond `--max-blob-bytes`; blobs above `--max-commit-diff-bytes` are refused with 413) |
| `raw/<path>` | raw blob (same 413 limit) |
| `refs.html` | branches and tags |
| `atom.xml`, `tags.xml` | feeds (`--public-url` makes the links absolute) |
| `style.css`, `logo.png`, `favicon.png` | embedded stagit assets, or from `--assets-dir` |
| `<prefix>/healthz`, `<prefix>/metrics`, `<prefix>/robots.txt` | health, Prometheus metrics, robots policy (`--robots allow`) |

### Options

Every flag can also be set through `STREAGIT_<NAME>` (e.g. `STREAGIT_ROOT`).
Options that default to on take an explicit value (`--allow-git-paths false`,
`--compression false`); `--files-line-counts` is a plain switch.

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
| `--allow-git-paths true\|false` | true | direct `<name>.git/...` URLs |
| `--link-refs true\|false` | true | link ref names on refs.html to `log.html?ref=` |
| `--robots` | `disallow` | robots.txt policy |
| `--nice` | 0 | process niceness |
| `--compression true\|false` | true | gzip responses |
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
  A client that stops reading cannot hold a render slot: once the render
  deadline passes without the stream being consumed, the render is dropped
  (`streagit_slow_clients_total` in `/metrics`).
- `refs.html` and `tags.xml` only read object headers while peeling refs
  (annotated tags use the `packed-refs` peel lines), so a repository with tens
  of thousands of tags costs milliseconds, not a commit load per tag.
- stagit's "Diff is too large, output suppressed." thresholds apply (1000 files
  or 100k lines); additionally single blobs above `--max-diff-blob-bytes` and
  commits above `--max-commit-diff-bytes` are listed but not diffed.

## Security

- streagit has **no authentication**. Every repository below `--root` is
  readable by anyone who can reach the port – all pages, `raw/` blob
  downloads, and `<prefix>/metrics`. Only point `--root` at repositories you
  are willing to publish to that audience.
- The default `--listen` is `127.0.0.1:8081`. To expose it, put a reverse
  proxy with TLS (and authentication, if the repositories are private) in
  front; streagit only uses relative links, so no `X-Forwarded-*` setup is
  needed.
- In Docker, streagit must listen on `0.0.0.0` *inside* the container so a
  published port reaches it. Publish the port on loopback
  (`-p 127.0.0.1:8081:8081`), as in the examples, unless the host's firewall
  or a proxy restricts access.
- `robots.txt` is served under the prefix (`<prefix>/robots.txt`, disallow
  all by default), but crawlers only read `/robots.txt` at the host root –
  map it there in the proxy if streagit runs under a prefix.
- Paths are resolved strictly below `--root`: `..`, encoded slashes and
  symlinks that escape the root are refused; only git's own files and
  `description`/`url`/`owner` are read from disk.

## Integration example: gitbackup

[gitbackup](https://github.com/MarvAmBass/gitbackup) mirrors repositories
into the slot layout above and used to run stagit after every change. With
streagit it starts `streagit --listen 127.0.0.1:8081 --root <data> --prefix
/browse --public-url <public URL of /browse> --cache-dir <state>/streagit`
next to its own web server and reverse-proxies `/browse/.../html*/...` (and
`.git/...` paths) to it; everything else under `/browse/` stays a file
server. The landing pages link `html/log.html`, `html.v1/log.html`,
`html.wiki/log.html` and so on as before. The same pattern works behind any
web server that can proxy by path.

## Building

No toolchain on the host is needed:

```sh
# static musl binary (native architecture of the builder)
docker run --rm -v "$PWD:/src" -w /src rust:alpine sh -c \
  'apk add musl-dev && cargo build --release --locked'
# -> target/release/streagit

# container image (x86_64; omit --platform for the builder's architecture)
docker build --platform linux/amd64 -t streagit .
docker run --rm -p 127.0.0.1:8081:8081 -v /path/to/data:/data:ro streagit \
  --listen 0.0.0.0:8081 --root /data --prefix /browse
```

`rust:alpine` targets musl natively, so the binary is fully static and runs on
any `alpine:3` (or `scratch`) image of the same architecture. The image's
default command is `--listen 0.0.0.0:8081 --root /data --prefix /browse`; the
license files are in `/usr/share/licenses/streagit/`.

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

Measured on an Apple Silicon laptop in Docker (Docker Desktop, arm64
`rust:alpine` container, repositories on the container file system, stagit
1.3 and streagit in the same setup); numbers are indicative, a low-power
server with spinning disks will be slower.

ComfyUI mirror, older snapshot (3,506 commits, 432 files in HEAD, 58 tags,
80 MB packs):

| | time |
|---|---|
| stagit 1.3: full render, all 3,506 commit pages + 432 file pages (162 MB) | **7–8 s** |
| streagit: `log.html` (100 rows, 100 diffs, cold) | 49 ms |
| streagit: `log.html` (cached) | < 1 ms |
| streagit: `log.html` with `If-None-Match` (304) | < 1 ms |
| streagit: largest commit page (79 files, 400k lines changed), cold | 106 ms (25 MB page; 0.41 s gzipped to 4.1 MB) |
| streagit: `files.html` cold | 1 ms |
| streagit: `refs.html` cold | 1 ms |
| streagit: whole history through log pages (every diffstat once) | 2.0 s (35 pages) |

Peak RSS during the whole-history walk was 185 MB (render cache 64 MiB,
four render threads with 8 MiB object caches, diffstat cache, plus the
memory-mapped pack files). Run `scripts/bench.sh` to reproduce.

Larger repositories and a newer ComfyUI snapshot (6.2k commits), same setup
(`--render-threads 2`, one server process serving all three, cold caches;
CPU = process CPU time per request):

| repository | page | cold | cached | CPU |
|---|---|---|---|---|
| git.git (82.5k commits, 4.9k files, 1k tags) | `log.html` | 153 ms | < 1 ms | 160 ms |
| | `log.html?from=` commit #41,256 | 308 ms | 1 ms | 300 ms |
| | largest commit of the last 4,000 (2.4 MB page) | 13 ms | 13 ms | 20 ms |
| | `files.html` (764 KB) | 10 ms | 1 ms | ~0 |
| | `refs.html` | 6 ms | 1 ms | 10 ms |
| llama.cpp (11.7k commits, 3.8k files, 8k tags) | `log.html` | 279 ms | < 1 ms | 290 ms |
| | `refs.html` (966 KB, 8,016 tags) | 62 ms | 2 ms | 70 ms |
| | largest commit of the last 4,000 (6.8 MB page) | 22 ms | 22 ms | 20 ms |
| | whole history through 117 log pages | 33 s | 20 ms | 33 s |
| ComfyUI (6.2k commits) | `log.html` | 85 ms | < 1 ms | 70 ms |

stagit 1.3 full renders of the same snapshots: ComfyUI (6.2k commits) 10 s
(341 MB of HTML), llama.cpp 61 s (1.1 GB), git.git 192 s (2.4 GB). After the
walks the process's RSS was about 550 MB, but only 84 MB of it was memory
streagit allocated itself (anonymous memory: caches, buffers); the remaining
~470 MB were memory-mapped pack files, which are file-backed and reclaimable
by the kernel under memory pressure.

## Fidelity and deviations from stagit

The page layout, ids and classes are stagit's; the golden test pins them. On
purpose streagit differs in a few places (all listed in `SPEC.md`):

- the description is trimmed (stagit keeps its trailing newline, which ends
  up in `<title>`); the repository name defaults to the slot name instead of `repo`;
  the Atom feed title uses that name instead of `repo.git`;
- `log.html` is paginated (`?from=`), `refs.html` names link to `log.html?ref=`;
- `files.html` shows byte sizes unless `--files-line-counts` is on;
- a tag that points at a tree is skipped instead of truncating the tag list;
- blobs and diffs have size limits; diffs use imara-diff's Myers, which can
  pick different but equivalent hunks for ambiguous inputs (counts match).

## License

MIT, see `LICENSE`.

`assets/style.css`, `assets/logo.png` and `assets/favicon.png` are copied
unchanged from stagit 1.3, and the generated page layout follows stagit's
output; stagit is MIT/X Consortium License, (c) 2015-2026 Hiltjo Posthuma,
its license text is in `LICENSE-stagit`. No stagit C code is used; the output
format is re-implemented.

Binary and Docker distributions include third-party Rust crates under the MIT,
Apache-2.0, MPL-2.0, BSL-1.0, BSD-3-Clause, Zlib and Unicode-3.0 licenses –
see `THIRD-PARTY-LICENSES.md` (regenerate with
`scripts/third-party-licenses.sh` after dependency updates). The Docker image
contains all three license files in `/usr/share/licenses/streagit/`.

`SPEC.md` is the original design document; where it differs from the code,
its §17 and the code are authoritative.
