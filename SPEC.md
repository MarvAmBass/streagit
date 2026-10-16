# streagit – implementation specification

**streagit** is an on-demand, streaming, stagit-compatible git web renderer
written in Rust. It serves the pages that [stagit](https://codemadness.org/stagit.html)
would pre-render (`log.html`, `files.html`, `refs.html`, `commit/<id>.html`,
`file/<path>.html`, `atom.xml`, `tags.xml`) straight from bare repositories,
at request time, over HTTP. Nothing is written to disk; results are cached in
memory keyed by immutable git object ids.

It exists so that [gitbackup](https://github.com/MarvAmBass/gitbackup) can stop
running `stagit` over hundreds of mirrored repositories after every change. Most of
the pre-rendered pages are never viewed; see §2 for measurements.

This document is written for an implementer who has **not** read stagit. It
reproduces stagit 1.3's exact output structure (§4) and defines everything
else (§5–§13). Where streagit deliberately deviates from stagit, the deviation
is marked **DEVIATION**.

Status: specification, v1. Target: stagit 1.3 (git://git.codemadness.org/stagit,
tag `1.3`; at the time of writing `master` == `1.3`, zero commits ahead).

---

## 1. Context: gitbackup's data layout

gitbackup keeps everything under one data root (`/data/data` in its
container, `--root` for streagit). Repositories are bare git directories
anywhere below the root. The folder of one repository is called a **slot**:

```
<root>/
  github.com/<owner>/repos/<name>/              slot
      repo.git                                  current mirror (bare)
      repo.v1.git  repo.v2.git                  frozen states before upstream rewrites
      wiki.git     wiki.v1.git
      html/  html.v1/                           stagit output (what streagit replaces)
      index.html                                gitbackup's landing page (links html/log.html …)
      releases/…
      forks/<owner>/repo.git                    forks with own commits – same layout, nested
  github.com/<owner>/gists/<id>/repo.git
  github.com/<owner>/forks/<name>/repo.git
  github.com/<user>/stars/<owner>/<name>  ->    symlink to ../../../<owner>/repos/<name> (inside root)
  gitlab.com/<group>/<sub>/repos/<project>/repo.git
  git.sr.ht/~user/repos/<name>/repo.git
```

Facts streagit must deal with:

- **Bare repos only**, refs are packed (`packed-refs`, written by gitbackup
  with `git pack-refs --all`; after a fetch there may also be loose refs
  under `refs/`). HEAD is a symbolic ref to a branch.
- Each repo contains the plain-text files `description`, `owner`, `url`
  (one line each, trailing newline) written by gitbackup. stagit reads
  `description` and `url`; `owner` is only used by stagit-index.
- **Preserved refs**: gitbackup stores old states as
  `refs/gitbackup/versions/<stamp>/heads/...` and `.../tags/...` inside
  `repo.git`. These must be **ignored**: streagit only looks at
  `refs/heads/*` and `refs/tags/*` (and HEAD). This matches stagit, which
  filters with `git_reference_is_branch() || git_reference_is_tag()`.
- **Versions and forks borrow objects** via `objects/info/alternates` with a
  **relative** path (e.g. `../../repo.git/objects`), relative to the
  `objects` directory of the repo that contains the file. Alternates can
  nest (a fork's version → the fork → the parent). The git library must
  resolve relative alternates, recursively.
- Sizes: hundreds of repos; some are huge (home-assistant/core ~ 100k commits,
  llama.cpp, Linux-sized repos are possible), some have 10k+ tags; single
  commits can touch thousands of files (ComfyUI has a commit whose stagit
  page is 30 MB).
- gitbackup serves the data root read-only at `/browse/` with Go's
  `http.FileServer`; its landing pages link `html/log.html`,
  `html/files.html`, `html/refs.html`, `html.v1/log.html` etc. (relative to
  the slot). Its dashboard links `/browse/<slot>/html/log.html`.
- The host is an Alpine Linux container (musl), a low-power CPU, spinning
  disks. CPU and disk seeks are precious; memory is moderate.

---

## 2. What stagit does and where the CPU goes

### 2.1 What stagit generates

`stagit [-c cachefile | -l commits] [-u baseurl] repodir`, run inside the
output directory, writes:

| file | content |
|---|---|
| `log.html` | one row per commit reachable from HEAD (reverse chronological), with files/+/- columns (**needs a full diff of every commit**) |
| `commit/<oid>.html` | one page per commit: header, diffstat, full diff |
| `files.html` | flat list of **every** file in HEAD's tree with mode, path, size in lines (`NL`, needs the blob read) or bytes |
| `file/<path>.html` | one page per file in HEAD: content with line numbers |
| `refs.html` | branches and tags (name, last commit date, author) |
| `atom.xml` | last 100 commits of HEAD |
| `tags.xml` | one entry per tag |

Commit pages are skipped when the file already exists (`access(path)`), but
the diff is still computed because `log.html` needs the counts. `-c
cachefile` stores the HEAD id plus the rendered log rows; the next run stops
walking at the cached HEAD. gitbackup uses `-c` and drops the cache
(`full=true`) whenever history was rewritten, i.e. a full re-render of every
commit. Frozen versions (`html.vN/`) are rendered once, fully.

### 2.2 Measurements

Benchmark: ComfyUI mirror as a bare repo (3,506 commits, 236 merges, 432
files in HEAD, 58 tags, 80 MB packs), stagit 1.3 built against libgit2
1.9.7 in `alpine:3`, instrumented with per-phase timers, repo copied to the
container's own filesystem (Docker Desktop on an Apple-Silicon host; the NAS
will be slower, especially cold-cache on spinning disks).

| run | log+commits | files | refs | atom+tags | total |
|---|---|---|---|---|---|
| 1. cold, no cache | 10.02 s | 0.49 s | 0.000 s | 0.003 s | 10.5 s |
| 2. cold, `-c cache` (first run writes it) | 11.86 s | 0.68 s | 0.000 s | 0.004 s | 12.6 s |
| 3. rerun, unchanged, `-c cache` | 0.004 s | 0.66 s | 0.001 s | 0.005 s | 0.68 s |
| 4. rerun, **commit pages exist, no cache** | 12.21 s | 0.27 s | 0.000 s | 0.002 s | 12.5 s |

Output: 3,506 commit pages + 432 file pages = **162 MB** of HTML for an 80 MB
repo; `log.html` alone is 1.0 MB (3,509 rows); the largest commit page is
30.5 MB (a commit touching generated JS: 12,495 insertions, 16,831
deletions – below stagit's "too large" cut-off of 100,000).

Conclusions:

1. **Diffing is ~100 % of the cost.** Run 4 (all commit pages already exist,
   nothing written except `log.html`) costs the same as run 1. Writing 162 MB
   of HTML is noise; computing `git_diff_tree_to_tree` + rename detection +
   `git_patch_from_diff` for every commit is what burns the CPU – and it is
   only needed for the three little numbers per row in `log.html`.
   ≈ 2.9 ms/commit here; expect 5–10 ms on the NAS.
2. `files.html` re-reads **every blob in HEAD** on **every run** (even an
   incremental one) to count lines: 0.5 s for 432 files; proportionally
   worse for repos with tens of thousands of files, and it happens on every
   change.
3. Everything else (refs, atom) is negligible for this repo. With 10k+ tags,
   `getrefs()` loads one commit per tag (still cheap, ~ms per 1000).
4. The cache only helps for fast-forward updates. Any rewrite, any new
   version, any change to stagit itself → full re-render. For hundreds of repos this
   is hours of CPU for pages nobody opens.

What streagit changes: the only work done is for the page requested. A log
page costs `page_size` diffs (≈ 50 × 3–10 ms) the first time and nothing the
second time (diffstat cache keyed by commit id); a commit page costs one
diff; a file page one blob read; nothing is ever done for pages that are
not requested.

---

## 3. Architecture

```
          gitbackup (Go, :8080)                       streagit (Rust, 127.0.0.1:8081)
   /browse/<slot>/html*/…  ── reverse proxy ──▶   GET /browse/<slot>/html/log.html
   /browse/<everything else>  file server         ├─ resolve URL → bare repo under --root (§5)
                                                  ├─ cache lookup / ETag (§8)
                                                  ├─ render in blocking pool, bounded (§9)
                                                  └─ stream chunked HTML (§7)
```

Single static binary (`streagit`), no runtime dependencies, no disk writes
(optional persistent diffstat cache, §8.4).

### 3.1 Crates

| concern | crate | notes |
|---|---|---|
| async runtime | `tokio` (rt-multi-thread, net, sync, time, signal) | |
| HTTP | `axum` 0.8 (on `hyper` 1.x) + `tower` + `tower-http` | routing, `Body::from_stream`, `ConcurrencyLimitLayer`, `TimeoutLayer`, `CompressionLayer` (gzip/br; the 30 MB page compresses ~10×), `TraceLayer` |
| git | **`gix`** (gitoxide), features `max-performance-safe`, `blob-diff`, `revision` (see §3.2) | pure Rust, static musl build with nothing but `rustup target add x86_64-unknown-linux-musl` |
| diff | `imara-diff` (re-exported through `gix::diff::blob`) | Myers / Histogram, `UnifiedDiff` helper for hunks |
| CLI/config | `clap` (derive) + env | |
| LRU | `lru` or hand-written (`hashbrown` + intrusive list); bytes-bounded | |
| time formatting | `jiff` (or `time`) – only fixed C-locale names needed | |
| escaping | hand-written (stagit's 5-char table; must match exactly) | |
| logging | `tracing` + `tracing-subscriber` (env filter) | |
| hashing cache keys | `blake3` or `xxhash-rust` | |
| errors | `anyhow` (bin), `thiserror` (lib) | |

### 3.2 git library: gix vs git2

Both were evaluated with a small program that reproduces stagit's log
workload (walk from HEAD, tree-diff vs. first parent with exact-match
rename+copy detection, line counts per delta) on the ComfyUI repo, built in
`rust:alpine` (cargo 1.99, gix 0.89.0, git2 0.21.0 / libgit2-sys 1.9.7).
See §2.2 for the stagit baseline (10.0 s for the same work incl. HTML).

| implementation | walk + diffstat of all 3,506 commits | first 50 commits (one log page) | deltas / +lines / -lines | binary |
|---|---|---|---|---|
| stagit 1.3 (libgit2 1.9.7, incl. writing all pages) | 10.0 s | – | 8,258 / 6,088,239 / 5,268,320 | – |
| `git2` 0.21 (libgit2 1.9.7, `Patch::line_stats`) | 6.36 s | 34 ms | 8,258 / 6,087,997 / 5,268,081 | 40 |
| `gix` 0.89 (`change.diff(cache).line_counts()`) | **2.58 s** | **21 ms** | 8,259 / 6,087,426 / 5,267,510 | 40 |

Both binaries static (`gix` one: 5 MB unstripped, "statically linked",
no C toolchain beyond `musl-dev`). The line totals differ by < 0.01 %:
stagit counts libgit2's "\ No newline at end of file" marker lines as
additions (+242 lines vs git2), and imara-diff's edit scripts differ from
xdiff's in a handful of ambiguous spots (the fixture-based golden test,
§12, pins the behaviour for the cases that matter). One delta more in gix
is a typechange/rename counted differently – to be checked on the fixture.
Per commit: gix ≈ 0.4–0.7 ms for typical commits on this machine; the
whole budget of a 100-row log page is ~50 ms warm, which is why keeping
stagit's +/- columns is affordable (§6.2, §8.3).

gix API notes from the experiment (gix 0.89): `tree.changes()?.options(|o|
o.track_rewrites(Some(Rewrites{..})))` then
`.for_each_to_obtain_tree(&new_tree, |change| -> gix::Result<Action> {..})`
where `Action` is `ControlFlow`-like (`Action::Continue(())`); the callback
**also receives tree (directory) and submodule entries** – skip anything
with `!change.entry_mode().is_blob_or_symlink()` (calling `.diff()` on a
tree entry errors with "Can only diff blobs and links"). The walk must set
`.sorting(Sorting::ByCommitTime(Default::default()))` (newest first) –
the default is breadth-first. Line counts:
`change.diff(&mut cache)?.line_counts()?` → `Option<DiffLineStats>`,
`None` = binary; call `cache.clear_resource_cache_keep_allocation()` after
each delta. Hunks: `platform.lines(|hunk| ..)` or
`gix::diff::blob::UnifiedDiff::new(&diff, &input, consumer, ContextSize::symmetrical(3))`
on `imara_diff::diff(Algorithm::Myers, &input, ..)`; set the algorithm
explicitly (gix otherwise follows `diff.algorithm` from config).

Decision: **gix**, for these reasons:

- **Static musl build is trivial** (pure Rust). git2 works too but needs
  `cmake`/`cc` in the build image and compiles libgit2 from source
  (`libgit2-sys` vendored, `default-features = false` to avoid
  OpenSSL/libssh2); fine, but one more moving part on Alpine.
- **Alternates**: `gix-odb` reads `objects/info/alternates`, resolves
  relative entries against the `objects` directory and follows them
  recursively (with cycle protection) – exactly what gitbackup writes.
  libgit2 does the same (depth limit 5). Equal.
- **packed-refs**: `gix-ref` reads `packed-refs` (including the `peeled`
  lines, so tag→commit needs no tag-object read) and loose refs; it re-reads
  the file when its mtime changes, so a long-running server sees gitbackup's
  updates without reopening the repository. libgit2 also caches with
  refresh. Equal.
- **Dynamic object store**: gix's `Store` lazily loads pack indices and
  refreshes its view of `objects/pack` on a miss, so packs added by
  gitbackup's fetches (and packs rewritten by `git gc`/`repack`) are picked
  up by a long-lived handle. Important for a daemon; libgit2 needs the odb
  to be refreshed explicitly (`git_odb_refresh`) or the repo reopened.
- **Speed**: pack access, delta resolution and commit decoding are faster in
  gix; `rev_walk` with `ByCommitTime` sorting reproduces `git log` order.
  Diffs use imara-diff (Histogram/Myers), faster than xdiff for typical
  inputs.
- **Memory/safety**: no C heap, no `git_patch` that materialises whole
  patches; the diff of one file is computed from two blob buffers and a
  compact edit script, so streaming per file is natural.
- **Risk**: gix's API is pre-1.0 and churns between minor versions
  (observed: `Sorting::BreadthFirst` is the *default* – **must** set
  `ByCommitTime(NewestFirst)`; the hash algorithm is a cargo feature, keep
  default features on). Pin the exact version in `Cargo.toml` and keep all
  gix usage behind the `git` module (§10) so a swap to git2 would touch one
  module. The gix calls needed are few: open, head, rev_walk, find_commit /
  find_tree / find_header, `tree.changes().options(track_rewrites)
  .for_each_to_obtain_tree()`, `change.diff(&mut cache)` → line counts /
  unified diff, `tree.lookup_entry_by_path`, `references()`.

If the implementer hits a blocker in gix (e.g. a diff API gap), fall back to
`git2` with `default-features = false` and the Dockerfile addition
`apk add cmake make`; the output must not change (golden tests, §12).

---

## 4. stagit's output, exactly

This section is the contract for fidelity. The HTML is copied from
`stagit.c` (tag 1.3). `ESC(x)` is stagit's `xmlencode` (§4.1), `ESCL(x)`
is `xmlencodeline` (same, but `\r` and `\n` are dropped), `PCT(x)` is
`percentencode` (§4.1). `R` is the **relpath**, `""` for top-level pages,
`"../"` for `commit/*`, and one `"../"` per `/` in the file path plus one
for `file/` pages (`file/a/b.c.html` → `../../`). All dates: §4.2.

Note the quirks that are preserved on purpose: XHTML-style `/>` closers,
`<table>` inside `<pre>`, `</pre>` closing a `<pre>` opened elsewhere, no
`lang`, no `<tbody>` closers on the index, a trailing newline inside
`<title>` when the description file ends with one (streagit trims
metadata – **DEVIATION**, see §4.9).

### 4.1 Escaping

`xmlencode` escapes exactly these 5 characters, nothing else (no numeric
escaping of control characters, no UTF-8 validation – bytes are copied):

```
<  → &lt;      >  → &gt;      '  → &#39;      &  → &amp;      "  → &quot;
```

`xmlencodeline` = `xmlencode` + drops `\r` and `\n` (used for blob lines
and diff lines, which carry their own `\n`).

`percentencode` (RFC 3986, used only for `href` paths): a byte `c` is
written as `%XX` (uppercase hex) if `c < ','` (0x2C), `c >= 127`,
`':' <= c <= '@'`, `c == '['` or `c == ']'`.
Not encoded: `,` `-` `.` `/` `0-9` `A-Z` `\` `^` `_` `` ` `` `a-z` `{` `|` `}`
`~`. Everything else is encoded, including space, `#`, `%`, `?`, `+`
(0x2B < 0x2C), `;`, `=`, `<`, `>`, `"`, `'`, `&` and all non-ASCII. Input is a path; `/` passes
through. streagit must implement this byte-exactly (links must match what a
browser requests; the server percent-decodes incoming paths, §5.4).

Strings are C strings in stagit: output stops at the first NUL byte. gix
gives `&[u8]`/`BStr`; streagit writes bytes as-is (no UTF-8 validation;
HTTP body is bytes). Declare `charset=UTF-8` as stagit does.

### 4.2 Time formats

Given a git signature time `(t: unix seconds, offset: minutes)`:

| function | used for | format (C locale, English names) |
|---|---|---|
| `printtimeshort` | log rows, refs rows | `strftime("%Y-%m-%d %H:%M", gmtime(t))` – **UTC**, offset ignored |
| `printtime` | commit page `Date:`, atom `<content>` | `strftime("%a, %e %b %Y %H:%M:%S", gmtime(t + offset*60))` then ` +HHMM` / ` -HHMM` (`%e` = day **space-padded** to 2 chars: `Thu,  9 Nov 2023 17:57:51 -0500`) |
| `printtimez` | atom `<published>`, `<updated>` | `strftime("%Y-%m-%dT%H:%M:%SZ", gmtime(t))` |

`%a` = Sun..Sat, `%b` = Jan..Dec. Negative offsets: sign then
`abs(offset)/60`, `abs(offset)%60`, each `%02d`. Out-of-range times
(gmtime fails) print nothing.

### 4.3 Page header and footer (every HTML page)

`writeheader(title)`; `NAME` is the repo name (§4.9), `DESC` the
description, `URL` the clone url, `TITLE` the page title:

```html
<!DOCTYPE html>
<html>
<head>
<meta http-equiv="Content-Type" content="text/html; charset=UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>ESC(TITLE)[ - ESC(NAME)][ - ESC(DESC)]</title>
<link rel="icon" type="image/png" href="Rfavicon.png" />
<link rel="alternate" type="application/atom+xml" title="ESC(NAME_RAW) Atom Feed" href="Ratom.xml" />
<link rel="alternate" type="application/atom+xml" title="ESC(NAME_RAW) Atom Feed (tags)" href="Rtags.xml" />
<link rel="stylesheet" type="text/css" href="Rstyle.css" />
</head>
<body>
<table><tr><td><a href="../R"><img src="Rlogo.png" alt="" width="32" height="32" /></a></td><td><h1>ESC(NAME)</h1><span class="desc">ESC(DESC)</span></td></tr><tr class="url"><td></td><td>git clone <a href="ESC(URL)">ESC(URL)</a></td></tr><tr><td></td><td>
<a href="Rlog.html">Log</a> | <a href="Rfiles.html">Files</a> | <a href="Rrefs.html">Refs</a> | <a href="Rfile/.gitmodules.html">Submodules</a> | <a href="Rfile/README.md.html">README</a> | <a href="Rfile/LICENSE.html">LICENSE</a></td></tr></table>
<hr/>
<div id="content">
```

Rules: ` - NAME` only if both TITLE and NAME are non-empty; ` - DESC` only
if DESC non-empty. `NAME_RAW` in the feed titles is the **unstripped**
directory name (stagit prints `repo.git Atom Feed`; streagit uses NAME
there too – **DEVIATION**, harmless). The `<tr class="url">` row only if
URL is non-empty (not percent-encoded, only xml-escaped). The `Submodules`,
`README`, `LICENSE` links only when detected (§4.10), with the detected
file name in the href. Note the logo link is `../R` → for `html/log.html`
that is the slot folder, i.e. gitbackup's landing page; for
`html/commit/x.html` it is `../../` = also the slot. Keep exactly.

`writefooter`:

```html
</div>
</body>
</html>
```

### 4.4 log.html

Title `Log`. Body between header and footer:

```html
<table id="log"><thead>
<tr><td><b>Date</b></td><td><b>Commit message</b></td><td><b>Author</b></td><td class="num" align="right"><b>Files</b></td><td class="num" align="right"><b>+</b></td><td class="num" align="right"><b>-</b></td></tr>
</thead><tbody>
<tr><td>SHORTDATE(author)</td><td><a href="Rcommit/OID.html">ESC(SUMMARY)</a></td><td>ESC(author.name)</td><td class="num" align="right">FILES</td><td class="num" align="right">+ADDS</td><td class="num" align="right">-DELS</td></tr>
…one row per commit…
</tbody></table>
```

- Order: `git_revwalk` with default sorting from HEAD = **`git log`
  default order** (reverse chronological by commit time, all parents
  followed, merges included). Verified: stagit's row order on ComfyUI is
  identical to `git log` and `git log --date-order`, and differs from
  `--topo-order`. In gix: `rev_walk([head]).sorting(Sorting::ByCommitTime(
  CommitTimeOrder::NewestFirst))`.
- `SUMMARY` = `git_commit_summary`: first paragraph of the message with
  newlines collapsed to single spaces and leading/trailing whitespace
  trimmed (libgit2: lines up to the first blank line, joined by one space;
  trailing whitespace of each line trimmed). The `<a>` is omitted if
  the summary is NULL (empty message) – the cell is empty.
- Date cell empty if no author.
- `FILES` = number of deltas after rename detection (`ndeltas`), `ADDS` /
  `DELS` = summed added/deleted lines over all non-binary deltas (§4.6).
- With `-l N` stagit appends
  `<tr><td></td><td colspan="5">K more commits remaining, fetch the repository</td></tr>`.
  streagit replaces this with pagination rows (§6.2) – **DEVIATION**.

### 4.5 commit/<oid>.html

Title = commit summary. Body:

```html
<pre><b>commit</b> <a href="../commit/OID.html">OID</a>
<b>parent</b> <a href="../commit/PARENT.html">PARENT</a>
<b>Author:</b> ESC(author.name) &lt;<a href="mailto:ESC(author.email)">ESC(author.email)</a>&gt;
<b>Date:</b>   TIME(author)

ESC(MESSAGE)
<b>Diffstat:</b>
<table><tr><td class="M">M</td><td><a href="#h0">ESC(old.path)[ -&gt; ESC(new.path)]</a></td><td> | </td><td class="num">CHANGED</td><td><span class="i">++++</span><span class="d">--</span></td></tr>
…one row per delta…
</table></pre><pre>N file[s] changed, A insertion[s](+), D deletion[s](-)
<hr/><b>diff --git a/<a id="h0" href="../file/PCT(old.path).html">ESC(old.path)</a> b/<a href="../file/PCT(new.path).html">ESC(new.path)</a></b>
<a href="#h0-0" id="h0-0" class="h">ESC(HUNK_HEADER)</a> context line
<a href="#h0-0-3" id="h0-0-3" class="d">-deleted line
</a><a href="#h0-0-4" id="h0-0-4" class="i">+added line
</a> context line
…
</pre>
```

Details, in order:

1. `printcommit`: `parent` line only if the commit has a first parent
   (merges: **only the first parent** is shown and diffed). Author block
   only if author present (`Date:` followed by exactly 3 spaces). Message:
   `\n` + ESC(full message, as stored, incl. trailing newline) + `\n`.
2. If the diff failed (`deltas == NULL`) nothing else is printed.
3. If `filecount > 1000 || ndeltas > 1000 || addcount > 100000 || delcount > 100000`:
   print `Diff is too large, output suppressed.\n` and stop.
4. Diffstat row per delta `i`: status letter `A` added, `C` copied, `D`
   deleted, `M` modified, `R` renamed, `T` typechange; anything else prints
   `<td> ` without class (only the listed ones get `class="X"`). Path: old
   path, then ` -&gt; new path` if they differ. `CHANGED` = adds+dels of the
   delta. Bar: `+`×adds then `-`×dels, **scaled** when adds+dels > 78:
   `add = floor(78/changed*add)+1` (if add>0), same for del (so the bar is
   at most 80 chars). Binary deltas: 0/0, empty bar.
5. Summary line: `N file changed` vs `files`; `insertion`/`insertions`,
   `deletion`/`deletions` by count == 1. Note the broken nesting
   `</table></pre><pre>…` – keep it.
6. Per delta: the `diff --git` header line with both paths linked to
   `../file/<path>.html` (these links target HEAD's file page, which may
   not exist for old paths – stagit doesn't care; streagit returns 404
   then). `id="hI"` on the first link. Binary delta → `Binary files differ.\n`
   and continue. Otherwise per hunk `j`: `<a href="#hI-J" id="hI-J"
   class="h">ESC(hunk header incl. its trailing \n)</a>`; then per line
   `k`: added (`old_lineno == -1`) → `<a href="#hI-J-K" id="hI-J-K"
   class="i">+` ESCL(content) `\n</a>`; deleted (`new_lineno == -1`) →
   same with `class="d">-`; context → ` ` ESCL(content) `\n`.
   Line index `k` counts all lines of the hunk including context. The
   "\ No newline at end of file" marker lines libgit2 emits have both
   line numbers -1 and are rendered as additions (`+\ No newline…`) and
   even counted in `addcount`; streagit renders them as a plain context
   line ` \ No newline at end of file` and does not count them –
   **DEVIATION** (acceptable).

Diff options (`commitinfo_getstats`):

- `git_diff_tree_to_tree(parent_tree or NULL, commit_tree)` with
  `GIT_DIFF_DISABLE_PATHSPEC_MATCH | GIT_DIFF_IGNORE_SUBMODULES |
  GIT_DIFF_INCLUDE_TYPECHANGE`; root commit → diff against empty tree.
- `git_diff_find_similar` with `GIT_DIFF_FIND_RENAMES | GIT_DIFF_FIND_COPIES
  | GIT_DIFF_FIND_EXACT_MATCH_ONLY`: renames and copies are detected only
  when the blob id is identical (no similarity scoring). Copies are looked
  for among files that are themselves modified in the commit (libgit2's
  `GIT_DIFF_FIND_COPIES` default source set). In gix:
  `Rewrites { copies: Some(Copies { source: CopySource::FromSetOfModifiedFiles, percentage: None }), percentage: None, limit: 1000, track_empty: false }`.
- Context lines: 3, interhunk: 0 (libgit2 defaults). Algorithm: Myers
  (xdiff). imara-diff's Myers may pick different but equivalent hunks for
  ambiguous inputs; counts should normally agree, byte-identical hunks are
  not required (§12).
- Submodule entries (tree entries of kind commit) are ignored in diffs.
- Binary detection for a delta (libgit2 `git_str_is_binary` on the first
  8000 bytes of either side, plus "file > max_size(512 MB)"): §4.11.
- Per-delta counts: a line with `old_lineno == -1` is an addition, with
  `new_lineno == -1` a deletion, counted only for non-binary deltas.

### 4.6 files.html

Title `Files`. Body:

```html
<table id="files"><thead>
<tr><td><b>Mode</b></td><td><b>Name</b></td><td class="num" align="right"><b>Size</b></td></tr>
</thead><tbody>
<tr><td>-rw-r--r--</td><td><a href="Rfile/PCT(path).html">ESC(path)</a></td><td class="num" align="right">151L</td></tr>
<tr><td>m---------</td><td><a href="Rfile/.gitmodules.html">ESC(path)</a> @ OID7</td><td class="num" align="right"></td></tr>
…
</tbody></table>
```

- Recursive walk of HEAD's tree, **in tree order** (entries sorted as git
  stores them: by name, with directories compared as `name/`), depth
  first, directories not listed, `path` = full path from the root.
- Mode string from the tree entry mode via `filemode()` (type char +
  rwx bits of the octal mode): `-rw-r--r--` (100644), `-rwxr-xr-x`
  (100755), `l---------` (120000 symlink – no permission bits), and for
  submodules (160000) the literal `m---------` row with the 7-char
  abbreviated commit id after ` @ ` and an empty size cell. (stagit only
  prints the submodule row when the commit object is *absent* from the
  repo – the normal case for a mirror; if it happens to be present the
  entry is silently skipped. streagit always prints the row.)
- Size cell: `NL` (line count, §4.7) if the blob is text and has > 0
  lines, else `NB` (raw byte size; binary files and empty files).
  **DEVIATION** (§6.4): streagit shows `NB` by default and `NL` only when
  `--files-line-counts` is on (requires reading every blob).

### 4.7 file/<path>.html

Title = file **name** (last path component). Body:

```html
<p> ESC(filename) (SIZEB)</p><hr/><pre id="blob">
<a href="#l1" class="line" id="l1">      1</a> ESCL(line 1)
<a href="#l2" class="line" id="l2">      2</a> ESCL(line 2)
…
</pre>
```

or, for binary blobs: `<p> ESC(filename) (SIZEB)</p><hr/><p>Binary file.</p>\n`.

- Line number format `%7zu` (right-aligned, width 7, space padded), then
  one space, then the line content without its `\n` (also without `\r`),
  then `\n`. Lines are split on `\n`; a final line without trailing
  newline is still numbered; an empty blob yields `<pre id="blob">\n</pre>`.
  The line count returned (`lc`) is what files.html prints as `NL`.
- `SIZE` is the raw blob size in bytes, e.g. `(25003B)`.
- Symlinks (mode 120000) are blobs whose content is the target path; stagit
  renders them like any file. Keep.

### 4.8 refs.html

Title `Refs`. For each of the two groups (branches, then tags), **only if
the group is non-empty**:

```html
<h2>Branches</h2><table id="branches"><thead>
<tr><td><b>Name</b></td><td><b>Last commit date</b></td><td><b>Author</b></td>
</tr>
</thead><tbody>
<tr><td>ESC(shorthand)</td><td>SHORTDATE(author)</td><td>ESC(author.name)</td></tr>
…
</tbody></table><br/>
<h2>Tags</h2><table id="tags"><thead>
… same …
</tbody></table><br/>
```

- Refs considered: `refs/heads/*` and `refs/tags/*` only. Symbolic refs
  are resolved; each ref is peeled to a commit (annotated tags → the
  tagged commit; packed-refs `^` peel lines can be used). A tag that
  peels to a non-commit (tree/blob) makes stagit **stop** iterating (bug);
  streagit skips it – **DEVIATION**.
- Sort: branches before tags; within a group by **author** time
  descending, then by shorthand name ascending (`strcmp`).
- `shorthand` = ref name without `refs/heads/` / `refs/tags/`.

### 4.9 Repository metadata

- `NAME`: stagit uses the basename of the repo dir with a trailing `.git`
  stripped. For gitbackup's layout every repo would be called `repo`
  (which is what the pre-rendered pages show today). **DEVIATION**:
  streagit's default name is the slot's last path component (the
  repository name, e.g. `myproject`); frozen versions keep the plain name
  (the description, which gitbackup sets per version, and the `html.vN`
  URL identify them). Configurable: `--name-from slot|dir|owner-slot`
  (`owner-slot` → `owner/myproject`, owner = contents of the `owner`
  file if present, else the directory two levels up the slot). Default
  `slot`.
- `DESC`: first line of `<repo>/description` (stagit: `fgets` into a 255
  byte buffer, trailing newline **kept**, which leaks into `<title>`).
  streagit: first line, trimmed, max 255 bytes – **DEVIATION**.
- `URL`: first line of `<repo>/url`, newline stripped (stagit), max 1023
  bytes.
- `owner`: not used by stagit pages (only stagit-index); streagit reads it
  for `--name-from owner-slot` and the index page (§6.6).
- stagit falls back to `.git/description` etc. for non-bare repos; streagit
  only supports bare repos (gitbackup) but may also accept
  `<dir>/.git/…` (cheap, optional).

### 4.10 README / LICENSE / submodules detection

Looked up in HEAD's tree, first match wins, must be a blob:

- LICENSE: `LICENSE`, `LICENSE.md`, `LICENSE.txt`, `COPYING`
- README: `README`, `README.md`
- Submodules: `.gitmodules`

The menu links `file/<name>.html`. (Case-sensitive, top-level only.)

### 4.11 Binary detection

libgit2 `git_str_is_binary` over the first 8000 bytes (`git_blob_is_binary`
for file pages; the diff driver for deltas, applied to each side):

```
skip a UTF-8 BOM; if a UTF-16/UTF-32 BOM is found → binary
for each byte c:
   if (c > 0x1F && c != 127) || c == '\b' || c == 0x1B || c == 0x0C: printable++
   else if c == 0: → binary
   else if !isspace(c): nonprintable++
binary = (printable >> 7) < nonprintable
```

streagit implements exactly this (`is_binary(&bytes[..min(8000, len)])`).
Additionally a blob larger than `--max-blob-bytes` is treated as "too
large" (§6.5), a deviation stagit doesn't have.

### 4.12 atom.xml

```xml
<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
<title>ESC(NAME), branch HEAD</title>
<subtitle>ESC(DESC)</subtitle>
<entry>
<id>OID</id>
<published>TIMEZ(author)</published>
<updated>TIMEZ(committer)</updated>
<title>[[ESC(TAG)] ]ESC(SUMMARY)</title>
<link rel="alternate" type="text/html" href="BASEURLcommit/OID.html" />
<author>
<name>ESC(author.name)</name>
<email>ESC(author.email)</email>
</author>
<content>commit OID
parent PARENT
Author: ESC(author.name) &lt;ESC(author.email)&gt;
Date:   TIME(author)

ESC(MESSAGE)
</content>
</entry>
…
</feed>
```

- `atom.xml`: the first **100** commits of the HEAD walk (same order as
  the log), `TAG` empty (no `[…] ` prefix). `<published>`/`<updated>` only
  if author/committer exist; `<title>` only if summary exists; `parent`
  line only with a parent; message block (`\n` + ESC(msg)) only if msg.
  `BASEURL` = `-u` value verbatim (stagit prints `href="%scommit/…"`, so
  the base URL must end with `/`); empty by default → relative link.
- `tags.xml`: one entry per **tag** (same order as refs.html), with
  `[tagname] ` prefixed to the title, commit data of the peeled commit.
  **DEVIATION**: streagit caps this at `--feed-entries` (default 100)
  newest tags.
- Content-Type `application/atom+xml; charset=utf-8`.

### 4.13 Static assets

Pages reference `Rstyle.css`, `Rlogo.png`, `Rfavicon.png`, i.e. files in
the per-repo HTML root (`<slot>/html/style.css`). gitbackup currently
copies its own `style.css` (light/dark variables, also styles its index
pages; selectors used: `#log #files #branches #tags #index #blob #content
td.num .desc .A .D span.i span.d pre a.h/.i/.d a.line tr.url`) and stagit's
`logo.png`/`favicon.png` into every html dir. streagit serves these three
names under every repo prefix from embedded defaults (stagit 1.3's
`style.css`, `logo.png`, `favicon.png`, MIT) or from `--assets-dir`
(gitbackup would point it at its own `style.css`). See §5.3.

### 4.14 stagit-index

`stagit-index repo…` writes an index page (`<table id="index">`: Name →
`<name>/log.html`, Description, Owner, Last commit = HEAD's author short
date). gitbackup does **not** use it (it generates its own landing/index
pages). streagit provides an optional equivalent (§6.6) but it is not
required for gitbackup.

---

## 5. HTTP interface

### 5.1 URL table

All routes are under `--prefix` (default `/`, gitbackup: `/browse`).
`<repo>` is a repo prefix as resolved by §5.2.

| method/path | page | cache key / ETag (§8) |
|---|---|---|
| `GET <repo>/` and `<repo>/index.html` | 302 → `log.html` | – |
| `GET <repo>/log.html` | log page 1 | `log:<tip>:<pagesize>` |
| `GET <repo>/log.html?from=<oid>` | log page starting at commit `<oid>` (inclusive) | `log:<tip>:<oid>:<pagesize>` |
| `GET <repo>/log.html?ref=<name>[&from=…]` | log of branch/tag `<name>` (**extension**, M2) | `log:<tip of ref>:…` |
| `GET <repo>/commit/<oid>.html` | commit page (`<oid>` = 40 hex) | `commit:<oid>` – immutable |
| `GET <repo>/files.html` | file list of HEAD | `files:<tree oid>` |
| `GET <repo>/file/<path>.html` | blob page for `<path>` in HEAD | `blob:<blob oid>:<name>` |
| `GET <repo>/raw/<path>` | raw blob, `text/plain` or `application/octet-stream` (**extension**, M3) | `<blob oid>` |
| `GET <repo>/refs.html` | branches and tags | `refs:<refs hash>` |
| `GET <repo>/atom.xml` | last 100 commits | `atom:<tip>` |
| `GET <repo>/tags.xml` | tags feed | `tags:<refs hash>` |
| `GET <repo>/style.css`, `logo.png`, `favicon.png` | static asset | content hash |
| `GET <prefix>/healthz` | `200 ok` | – |
| `GET <prefix>/` | optional index of all repos (§6.6), else 404 | – |
| anything else | 404 page | – |

`<tip>` = the resolved HEAD commit id (or the ref's tip). `<oid>` in URLs
must be exactly 40 lowercase hex chars (SHA-1; be ready for 64 if the repo
is SHA-256) – anything else is 404 without touching the repo. Short ids
are not supported (stagit never links them).

HEAD, OPTIONS: HEAD like GET without body. Everything else: 405.

### 5.2 Repo resolution (URL → bare repository)

Split the request path (after `--prefix`) into percent-decoded segments.
Walk the segments from the left; the **first** segment that matches one of
the patterns below ends the slot path and names the git directory:

| segment pattern | git directory in the slot | example |
|---|---|---|
| `html` | `repo.git` | `github.com/a/repos/b/html/log.html` → `github.com/a/repos/b/repo.git` |
| `html.v<N>` (N ≥ 1, no leading zeros) | `repo.v<N>.git` | `…/b/html.v2/files.html` → `…/b/repo.v2.git` |
| `html.<unit>` | `<unit>.git` | `…/b/html.wiki/log.html` → `…/b/wiki.git` |
| `html.<unit>.v<N>` | `<unit>.v<N>.git` | `…/b/html.wiki.v1/…` → `…/b/wiki.v1.git` |
| `<name>.git` (direct mode, if `--allow-git-paths`, default on) | that directory | `…/b/forks/x/repo.git/log.html` |

`<unit>` and `<name>` match `[A-Za-z0-9_.~+-]+` (gitbackup's safe set).
The remaining segments are the **page path** (§5.1). The slot segments and
the git dir are joined onto `--root`.

Forks live *inside* a slot (`…/repos/b/forks/x/repo.git`), and their pages
are `…/repos/b/forks/x/html/log.html`: the rule "first matching segment"
would wrongly stop at … no – `forks` and `x` don't match any pattern, so
the walk continues to `html`. Correct. But a repo named `html` would be
ambiguous (`…/repos/html/html/log.html`): the first `html` is at the
*slot-name* position. Resolution rule: try the **longest** candidate first
(scan from the right: the last segment matching a pattern such that the
resulting git dir exists wins). Implementation: collect all matching
segment positions, try from the rightmost; a candidate is accepted iff
`<root>/<segments..i>/<gitdir>/HEAD` exists. At most 2–3 stat calls.

### 5.3 Assets under repo prefixes

`style.css`, `logo.png`, `favicon.png` are served for any resolved repo
prefix *without* requiring the repo to exist? No: resolve the repo first
(cheap stat), 404 otherwise, so the asset routes can't be used to probe.
Response: `Cache-Control: public, max-age=86400`, strong ETag = blake3 of
the content, `Content-Type` `text/css; charset=utf-8` / `image/png`.

### 5.4 Path safety

- Percent-decode each segment; reject (404) a segment that is empty
  (`//`), `.`, `..`, contains NUL, `\`, or a `/` after decoding (`%2F`).
- The joined path must stay under `--root`: after joining, `canonicalize()`
  the git dir and require `starts_with(canonicalize(root))`. Symlinks
  inside the root (gitbackup's `stars/` links) are fine; a symlink that
  escapes the root is 404. The page path (`file/<path>`) never touches the
  filesystem – it's resolved inside the git tree.
- Never serve arbitrary files from disk. The only disk reads are
  `HEAD`/`description`/`url`/`owner` and git's own files via gix.
- Request path length limit 4096, segment count ≤ 64, query length ≤ 1024.
- Don't follow `objects/info/alternates` outside the root? gix follows
  whatever is in the file; gitbackup writes relative in-root paths. Accept
  (the files are gitbackup's, not user input).

### 5.5 Responses and headers

- `Content-Type: text/html; charset=utf-8` (pages), atom as above.
- `Transfer-Encoding: chunked` for rendered pages (no Content-Length),
  except cached pages (known length).
- `ETag`, `Cache-Control`, `Last-Modified` (commit pages: committer time) –
  §8.1. `Vary: Accept-Encoding` with compression.
- Status: 200, 304 (ETag match), 302 (repo root → log.html), 404 (repo,
  ref, object or path not found; bad oid syntax), 405, 413 (path too
  long), 429/503 (`Retry-After: 5`) when the render queue is full, 500
  (corrupt object / io error, logged), 504 (render timeout, only if
  nothing was sent yet; otherwise the page ends with a notice, §7.3).
- Error pages use the stagit header/footer (so they look like the site;
  the header is rendered from the repo metadata if the repo resolved) with
  `<p>` text: `404 Not Found: <what>`. For an unresolvable repo (no
  metadata, no per-repo assets) a minimal page with a small inline
  `<style>` is used.

---

## 6. Page semantics beyond stagit

### 6.1 HEAD resolution

`HEAD` → symbolic ref → commit. If HEAD is unborn/dangling (branch deleted
upstream but HEAD still points at it; gitbackup version repos set HEAD to
`main`/`master`/first head), fall back to: `refs/heads/main`,
`refs/heads/master`, then the first branch by name; if there is no branch
at all, pages render with an empty log/files ("empty repository" row) and
200. Tags-only repos: log of HEAD empty, refs.html lists the tags.

### 6.2 Log pagination

`--page-size N` (default 100, max clamp 500). `log.html` renders the first
N commits of the walk from the tip. The row after the last commit:

```html
<tr><td></td><td colspan="5"><a href="log.html?from=NEXT_OID">Next N commits →</a></td></tr>
```

(`class="pager"`? no extra classes needed; stagit's CSS already styles the
row.) `NEXT_OID` is the (N+1)-th commit of the walk (walk N+1, render N).
On `?from=<oid>`: push `<oid>` instead of the tip (`<oid>` must be a
commit reachable… not checked – any existing commit works, which is
harmless), render N, same "Next" row, plus a leading row
`<tr><td></td><td colspan="5"><a href="log.html">↑ newest</a></td></tr>`.
No "previous" link (would need the inverse walk); browsers have Back.
`?from=` with a non-commit or unknown id → 404.

Why not `?page=K`: skipping K·N commits means walking (decoding) K·N
commits every time; `from=` restarts the walk at the right place for free.
Optional `?page=K` support (walk-skip, bounded by `--max-skip`, default
10000 commits) may be added for crawlers/humans typing URLs; not required.

ETag/cache key: `log:<tip>:<from>:<N>` – the tip is included so that a page
`from=X` is invalidated when history is rewritten (the same `X` may be
unreachable later, or the metadata changed). Diffstat per row comes from
the diffstat cache (§8.3); a log page of a 100k-commit repo costs 100 diffs
once, then nothing.

`?ref=<name>` (M2): `<name>` is a branch or tag shorthand, resolved under
`refs/heads/` then `refs/tags/`, 404 otherwise. refs.html rows then link
their name to `log.html?ref=<name>` – **extension**, not in stagit (opt-in
via `--link-refs`, default on; the golden tests strip hrefs in refs rows).

### 6.3 Commit pages

Computed on demand (§7.2), stagit's thresholds apply (1000 files / 100k
lines → "Diff is too large, output suppressed."). Additional streagit
limits (**DEVIATION**, needed for bounded memory):

- A delta whose old or new blob is larger than `--max-diff-blob-bytes`
  (default 4 MiB) is rendered like a binary delta but with the text
  `Diff skipped, file too large (N bytes).` and counts 0/0 in the diffstat
  (still listed, status letter kept).
- If the total size of blobs loaded for one commit exceeds
  `--max-commit-diff-bytes` (default 64 MiB), the remaining deltas are
  rendered with `Diff skipped, commit too large.` The diffstat table still
  lists all deltas (counts 0/0 for skipped ones).

Merge commits: diff vs. first parent only (stagit). Root commit: diff vs.
empty tree (everything `A`).

### 6.4 Files page

`files.html` lists every blob in HEAD's tree recursively, like stagit,
capped at `--max-files` entries (default 20000); beyond that a final row
`<tr><td></td><td colspan="2">… N more files not shown</td></tr>` –
**DEVIATION**. The size cell shows bytes (`NB`) by default; with
`--files-line-counts` it shows `NL` for text blobs (requires reading every
blob; line counts are cached per blob id in the stats cache). Blob sizes
come from object headers only (`find_header`) – no blob is decompressed in
the default mode.

M3 extension: `tree/` and `tree/<dir>/` directory pages (same table, with
`d---------` rows linking into sub directories) for very large trees;
`files.html` stays the flat compatibility view.

### 6.5 Blob pages

`file/<path>.html`: strip the final `.html` once (a file literally named
`x.html` is served at `file/x.html.html`); percent-decoded path segments
are looked up in HEAD's tree (`tree.lookup_entry_by_path`), 404 if missing
or not a blob (a directory path → 404; submodule → 404). Symlink blobs
render their target text. Binary → `<p>Binary file.</p>`. Blobs larger
than `--max-blob-bytes` (default 2 MiB) render the first `--max-blob-bytes`
bytes (cut at the last `\n`) followed by `</pre><p>File truncated, N bytes
not shown.</p>` with a `raw/<path>` link if M3 is implemented –
**DEVIATION**. Streams line by line (§7).

### 6.6 Index page (optional, M3)

`GET <prefix>/` lists all repos below the root (`<table id="index">`, same
structure as stagit-index: name linking to `<slot>/html/log.html`,
description, owner, last commit date). Discovery = walking the root for
`*.git/HEAD` (cached for `--index-ttl`, default 10 min). gitbackup has its
own index and landing pages, so this is off by default (`--index`).

---

## 7. Rendering and streaming

### 7.1 Pipeline

```
request ─▶ resolve repo (stat) ─▶ resolve tip/object ids (cheap, ms)
        ─▶ compute cache key + ETag ─▶ If-None-Match? 304
        ─▶ render cache hit? serve bytes
        ─▶ acquire render permit (semaphore, §9) ─▶ spawn_blocking(render)
                 │ chunks (Bytes, ≤64 KiB) via bounded mpsc(8) ──▶ axum Body::from_stream ──▶ client
                 └ on completion: if total ≤ cache-max-entry, insert into render cache
```

All git/diff/HTML work happens in a `Render` function with the signature
`fn render(ctx: &RenderCtx, out: &mut dyn Out) -> Result<(), RenderError>`
where `Out` has `write(&[u8])`, `esc(&[u8])`, `escl(&[u8])`, `pct(&[u8])`,
`fmt_args`. `Out` buffers into a `BytesMut` and flushes 64 KiB chunks to
the channel (`blocking_send`); a closed channel (client gone) returns
`RenderError::Cancelled`, which aborts the render promptly. The first
chunk is flushed as soon as the header is written (`flush()` after
`writeheader`) so the browser shows the page skeleton immediately.

Memory: one render holds at most the chunk buffer + what the page needs
(§6.3/6.5 limits). With `--render-threads 2` worst case ≈ 2 × 64 MiB.

### 7.2 Commit page rendering order

The diffstat table precedes the diffs, but needs all counts. Procedure:

1. Walk the tree diff (with rewrites) collecting `Vec<Delta { status,
   old_path, new_path, old_id, new_id, old_mode, new_mode }>`; abort with
   "too large" when deltas > 1000.
2. For each delta: load both blobs (respecting §6.3 limits), binary check,
   compute the diff with imara-diff (`Algorithm::Myers`), keep
   `FileDiff { delta_idx, kind: Binary|Skipped|Text { old: Vec<u8>, new:
   Vec<u8>, hunks: Vec<Hunk{ old_range, new_range, header: String, lines:
   Vec<(Kind, Range)> }> }, adds, dels }`. Hunks reference line ranges into
   the kept blob buffers (no copy of the line text). Stop early
   (`TooLarge`) once `adds > 100000 || dels > 100000`.
3. Write header + diffstat + summary line, then stream the hunks.

For the **log page** only step 1–2's counts are needed; the `FileDiff`s
are not kept (counts go to the stats cache). Commit page and log use the
same `diff_commit(repo, commit, mode: CountsOnly | Full, limits)`.

Hunk header text: `@@ -A[,B] +C[,D] @@` + (git's "function context" –
libgit2 prints a heading from the nearest preceding line matching the
default funcname regex `^[[:alpha:]$_]`, i.e. a line starting with a letter,
`$` or `_`, truncated to 80 bytes) + `\n`. Implement the default funcname
rule (no `.gitattributes` drivers). Counts `,B` / `,D` are omitted when the
range length is 1 (unified diff convention); `-0,0` for an empty side.

### 7.3 Timeouts and cancellation

`--render-timeout` (default 30 s, log pages 60 s): the render thread checks
a deadline between deltas/rows/lines; on expiry it writes
`<p>Rendering aborted: time limit exceeded.</p>` + footer and returns
`Partial` (not cached). If the timeout hits before the first byte (can
only happen for log pages still in the diff phase – the header is written
before diffs start, so practically never), respond 504.

### 7.4 Writer hygiene

- Never format with `{}` of a `BStr` into the HTML without `esc`.
- Integer widths: `%7zu` → `format!("{:>7}", n)`.
- Diffstat bar characters are ASCII `+`/`-` bytes.

---

## 8. Caching

Four layers, all keyed by immutable ids, so there is no invalidation logic
beyond "the key changed":

### 8.1 HTTP (browser / proxy)

| page | ETag (strong, quoted) | Cache-Control |
|---|---|---|
| `commit/<oid>.html` | `"<oid>-<TV>"` | `public, max-age=86400` (content immutable per oid; metadata/README links in the header may change, so not `immutable`) |
| `file/<path>.html` | `"<blob oid>-<TV>"` | `public, max-age=60, must-revalidate` |
| `files.html` | `"<tree oid>-<TV>"` | same |
| `log.html…` | `"<tip>-<from>-<N>-<TV>"` | same |
| `refs.html`, `tags.xml` | `"<refs hash>-<TV>"` | same |
| `atom.xml` | `"<tip>-<TV>"` | same |
| assets | `"<content hash>"` | `public, max-age=86400` |

`TV` = template version, a short hash of the streagit version + options
that change output (`name-from`, `files-line-counts`), so an upgrade busts
browser caches. `If-None-Match` is checked **before** any render: for
commit/blob/tree pages only the id resolution (HEAD → commit → tree →
entry) runs, which is sub-millisecond on a warm repo.

### 8.2 Render cache (in memory)

`RenderCache`: LRU bounded by **bytes** (`--cache-bytes`, default 64 MiB)
and entry count (default 10000), key = the cache key of §5.1 (with TV),
value = `Bytes` of the full page (uncompressed; compression happens in
tower-http per response – optionally store gzip'd and serve pre-compressed
when the client accepts gzip, M3). Entries larger than
`--cache-max-entry` (default 1 MiB) are not stored (the 30 MB diff is
recomputed on demand – rare). Lookup is before the permit; a hit never
touches the render pool.

Log pages are cached by `tip`, so they drop out naturally when the repo is
updated; old-tip entries age out of the LRU.

### 8.3 Diffstat cache (in memory, persistent optional)

`StatsCache`: map `commit oid → (files: u32, adds: u32, dels: u32)`,
and (if `--files-line-counts`) `blob oid → lines: u32`. 20-byte key +
12-byte value, bounded by `--stats-entries` (default 2,000,000 ≈ 64 MiB
with overhead; 1M entries covers the whole history of the biggest repos).
Filled by every log and commit render; read by log rendering. Also
pre-fills from a commit render (the full diff knows the counts). Hit = no
diff needed for that row.

A log page therefore costs: N commit decodes (fast) + diffs only for
commits not yet in the cache.

### 8.4 Persistent stats (optional)

`--cache-dir <dir>`: the stats cache is persisted so that log pages stay
cheap across restarts and container rebuilds (the NAS restarts gitbackup on
every deploy). Format: one append-only file per repo
(`<cache-dir>/<blake3(canonical repo path)[..16]>.stats`, records of
`[oid 20][files u32][adds u32][dels u32]` little-endian = 32 bytes), loaded
on first use of the repo (bounded by `--stats-entries` overall; load the
tail if larger), appended under a mutex with `O_APPEND` writes of whole
records, fsync not required (a torn record at the end is detected by
length and dropped). No deletion needed: object ids never change meaning;
the file grows by 32 bytes per commit ever viewed in a log page. A
`streagit --stats-prune` subcommand could drop records whose commits are
unreachable, but it is not needed for v1.

Hashing the canonical path means `repo.v1.git` (same objects, different
dir) has its own file – acceptable (the objects are the same, so
optionally key by the repo's **object store identity**: the canonical
path of the last alternate, i.e. the base repo. Nice-to-have).

### 8.5 Repository handle cache

`RepoCache`: LRU of `--open-repos` (default 64) `gix::ThreadSafeRepository`
keyed by canonical git dir path, each with `object_cache_size` set
(default 16 MiB per repo? no – per *handle*: `to_thread_local()` per render
with `object_cache_size_if_unset(--object-cache-bytes, default 8 MiB)`).
A handle stays valid across gitbackup's fetches and `pack-refs` (gix
refreshes refs by mtime and the odb on miss). Repos whose directory
disappears (gitbackup renames `repo.git.tmp`→…; versions are created once)
return errors → evict and retry once.

`HEAD`, `description`, `url`, `owner` are read per request (4 small
reads; could be cached by mtime, not needed).

---

## 9. Concurrency and resource limits

- **Render pool**: `--render-threads` (default `max(1, cores/4)`,
  gitbackup NAS: 2). Implemented as a `tokio::sync::Semaphore` with that
  many permits around `spawn_blocking` (tokio's blocking pool is sized ≥
  that). A request waits for a permit at most `--queue-timeout` (default
  10 s) with at most `--max-queue` (default 32) waiters; beyond that → 503
  + `Retry-After: 5`. Cache hits and 304s bypass the pool entirely, so a
  crawler re-fetching known pages costs nothing.
- **Per-request limits**: §6.3, §6.5, §7.3.
- **Connection limits**: hyper defaults + `--max-connections` (default
  256) via `tower::limit::ConcurrencyLimitLayer` on the whole service;
  request body size 0 (GET only); header timeout 10 s.
- **Single-flight**: concurrent requests for the same cache key share one
  render (a `DashMap<Key, broadcast>`-style "in-flight" map; the second
  request waits for the first's result and serves it from the cache; if
  the first result isn't cacheable (too big), the second renders itself).
  Prevents a crawler hitting one huge commit page 10× in parallel.
- **Priority**: `--nice N` calls `setpriority(PRIO_PROCESS, 0, N)` at
  startup (Linux; via `libc`), and the recommended deployment also uses
  `ionice -c3`/`nice -n 10` in the entrypoint so the renderer never
  competes with gitbackup's fetches. Document both.
- **robots.txt**: served at `<prefix>/robots.txt` as
  `User-agent: *\nDisallow: /` by default (`--robots allow` to open);
  gitbackup is a private backup browser and crawlers are the main CPU
  risk. The reverse proxy must forward it (gitbackup currently has no
  robots.txt; it should proxy `/robots.txt` too, or serve its own).

---

## 10. Code layout

Single crate (binary + lib for tests), no workspace needed:

```
streagit/
  Cargo.toml                 [[bin]] streagit; [lib] streagit (so tests can call render fns)
  src/
    main.rs                  clap → Config → server::run
    config.rs                Config struct (all flags, env `STREAGIT_*`)
    server.rs                axum router, middleware stack, graceful shutdown (SIGTERM)
    http/
      resolve.rs             §5.2/5.4: path → RepoRef { git_dir, slot, kind, version } + page
      handlers.rs            one handler per page; ETag/304; cache; permit; spawn render
      stream.rs              ChunkWriter (Out impl) + mpsc → Body
      errors.rs              404/500/503 pages
      assets.rs              embedded style.css/logo.png/favicon.png, --assets-dir override
    git/
      repo.rs                RepoCache, open, metadata (name/description/url/owner), head resolution
      log.rs                 walk(tip, from, n) → Vec<CommitInfo>
      commit.rs              CommitInfo { id, parent, author, committer, summary, message }
      diff.rs                diff_commit(mode) → Delta/FileDiff, rewrites config, limits, funcname
      tree.rs                files listing (recursive, with modes & sizes), lookup_path
      blob.rs                is_binary, split lines, truncation
      refs.rs                branches/tags with peeled commit + author time, sort, refs hash
    render/
      out.rs                 Out trait, esc/escl/pct, fmt helpers
      header.rs              writeheader/writefooter
      log.rs  commit.rs  files.rs  blob.rs  refs.rs  atom.rs  index.rs
      time.rs                printtime / printtimeshort / printtimez
    cache/
      render.rs              bytes-bounded LRU
      stats.rs               StatsCache + persistent file
      inflight.rs            single-flight
  assets/style.css logo.png favicon.png     from stagit 1.3 (MIT)
  tests/
    fixtures.rs              builds fixture repos with `git` CLI at test time
    golden/…                 stagit 1.3 output for the fixture (checked in)
    golden_test.rs  http_test.rs  resolve_test.rs
  benches/log_page.rs        criterion (optional)
  Dockerfile  LICENSE  README.md  SPEC.md
```

Key types:

```rust
pub struct RepoRef { pub git_dir: PathBuf, pub slot: String, pub url_prefix: String }
pub struct Meta { pub name: String, pub description: Vec<u8>, pub url: Vec<u8>, pub owner: Vec<u8>,
                  pub readme: Option<&'static str>, pub license: Option<&'static str>, pub submodules: bool }
pub struct Sig { pub name: Vec<u8>, pub email: Vec<u8>, pub time: i64, pub offset_min: i32 }
pub struct CommitInfo { pub id: ObjectId, pub parent: Option<ObjectId>, pub author: Option<Sig>,
                        pub committer: Option<Sig>, pub summary: Option<Vec<u8>>, pub message: Vec<u8> }
pub struct Stats { pub files: u32, pub adds: u32, pub dels: u32 }
pub enum DeltaStatus { Added, Copied, Deleted, Modified, Renamed, TypeChange }
pub struct Delta { pub status: DeltaStatus, pub old_path: Vec<u8>, pub new_path: Vec<u8>,
                   pub old_id: Option<ObjectId>, pub new_id: Option<ObjectId> }
pub enum FileDiff { Binary, Skipped(SkipReason), Text { old: Vec<u8>, new: Vec<u8>, hunks: Vec<Hunk>, adds: u32, dels: u32 } }
pub struct Hunk { pub header: Vec<u8>, pub lines: Vec<(LineKind, (usize, usize))> }  // byte range into old/new
pub enum LineKind { Context, Add, Del, NoNewline }
pub trait Out { fn write(&mut self, b: &[u8]) -> R; fn esc(&mut self, b: &[u8]) -> R;
                fn escl(&mut self, b: &[u8]) -> R; fn pct(&mut self, b: &[u8]) -> R; fn flush(&mut self) -> R; }
```

Mapping of gix tree-diff changes to `DeltaStatus`: `Addition`→A,
`Deletion`→D, `Modification`→M (or T if the entry *kind* changed between
blob/symlink/commit – `INCLUDE_TYPECHANGE`), `Rewrite{copy:false}`→R,
`Rewrite{copy:true}`→C. Entries of kind commit (submodules) are dropped
before counting (`IGNORE_SUBMODULES`). Order of deltas: libgit2 sorts
deltas by path (old path for renames? by `new_file.path`? libgit2 sorts by
old path then new); gix yields tree order. Sort `Vec<Delta>` by
`old_path` bytes, then `new_path` – good enough for the golden test
(§12 compares as sets anyway).

---

## 11. Configuration

Flags (clap), each also readable from `STREAGIT_<NAME>` env:

| flag | default | meaning |
|---|---|---|
| `--listen ADDR` | `127.0.0.1:8081` | bind address (TCP); `--listen unix:/run/streagit.sock` optional |
| `--root DIR` | required | data root containing the slots |
| `--prefix PATH` | `/` | URL prefix stripped before resolution (`/browse` for gitbackup) |
| `--public-url URL` | – | absolute URL of `--prefix` as seen by users (`https://example.org/browse`), used for atom `href`s; if unset, atom links are relative (stagit's default) |
| `--page-size N` | 100 | log rows per page |
| `--render-threads N` | cores/4, min 1 | concurrent renders |
| `--max-queue N` / `--queue-timeout` | 32 / 10s | |
| `--render-timeout` | 30s | |
| `--cache-bytes` / `--cache-max-entry` | 64MiB / 1MiB | render cache |
| `--stats-entries` | 2,000,000 | diffstat cache entries |
| `--cache-dir DIR` | – | persistent stats (§8.4) |
| `--open-repos N` / `--object-cache-bytes` | 64 / 8MiB | |
| `--max-blob-bytes` | 2MiB | blob page truncation |
| `--max-diff-blob-bytes` / `--max-commit-diff-bytes` | 4MiB / 64MiB | |
| `--max-files` | 20000 | files.html rows |
| `--files-line-counts` | off | `NL` sizes like stagit |
| `--feed-entries` | 100 | atom/tags entries |
| `--name-from slot\|dir\|owner-slot` | slot | §4.9 |
| `--assets-dir DIR` | embedded | override style.css/logo.png/favicon.png |
| `--allow-git-paths` | on | direct `<x>.git` URLs (§5.2) |
| `--link-refs` | on | refs.html names link to `log.html?ref=` |
| `--index` / `--index-ttl` | off / 10m | §6.6 |
| `--robots allow\|disallow` | disallow | |
| `--nice N` | 0 | |
| `--compression` | on | gzip/br |
| `--log-level` | info | `RUST_LOG` compatible |

Behind a reverse proxy: pages use **relative** links only, so no
`X-Forwarded-*` handling is needed; `--public-url` is the only absolute
URL. gitbackup already has `GITBACKUP_BROWSE_URL` for this.

---

## 12. Testing

1. **Fixture repos** built at test time with the `git` CLI
   (`tests/fixtures.rs`): a bare repo with ≥ 30 commits covering: root
   commit, merge (first-parent diff), rename (exact), copy, typechange
   (file→symlink), binary file add/modify, submodule entry, non-UTF-8
   bytes in a path and in content, CRLF file, file without trailing
   newline, empty file, 10k-line file, a path with spaces/`#`/`%`/`+`/
   unicode, a `README.md`, `LICENSE`, `.gitmodules`, annotated and
   lightweight tags (one pointing at a tree), two branches, packed refs,
   `description`/`url`/`owner` files, a `repo.v1.git` with relative
   alternates and a `forks/x/repo.git` with nested alternates, and a
   `refs/gitbackup/versions/…` ref that must be ignored.
2. **Golden tests**: stagit 1.3's output for the fixture is generated once
   (script `tests/golden/gen.sh`, runs stagit in `alpine:3` Docker) and
   checked in. Tests render each page with streagit and compare after
   normalisation: trim description newline, drop streagit's pagination/
   "newest" rows, drop `href` on refs names, ignore `Atom Feed` title
   name, treat `NB`/`NL` size cells as equal when `--files-line-counts`
   is off (the golden run has it on), ignore hunk-internal differences
   (compare per-delta adds/dels and the set of hunk headers' ranges
   loosely: equal line counts; and compare the full text for the
   fixture's simple diffs where Myers is unambiguous). Commit pages: the
   set of commit ids in log.html and their file/+/- counts must match
   stagit exactly (this is the property that matters most to users).
   files.html: identical row set and order. refs.html: identical.
   atom/tags: identical modulo caps.
3. **HTTP tests** (`axum::serve` on an ephemeral port or `tower::ServiceExt::oneshot`):
   resolution table (§5.2) incl. `html.v2`, `html.wiki`, nested forks,
   repo literally named `html`; path traversal (`..`, `%2e%2e`, `%2F`,
   symlink out of root → 404; symlink inside root → 200); ETag → 304;
   `?from=` paging walks the whole fixture history without gaps or
   duplicates; 404 for bad oid, missing file, directory path; a
   `Range`/HEAD request; concurrency limit → 503; a slow client
   disconnecting cancels the render (channel closed); render timeout
   notice; large blob truncation; binary detection table (§4.11 cases).
4. **Property tests** (proptest, small): `esc`/`pct` byte tables vs. a
   straightforward reference; `printtime` vs. `chrono`/`jiff` for random
   timestamps/offsets; `is_binary` vs. a C-translated reference.
5. **Benchmarks** (criterion, `cargo bench`, optional in CI): log page
   cold/warm, commit page for a big commit, files.html on a 20k-entry tree
   – run against a real repo path from `STREAGIT_BENCH_REPO`.
6. CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`,
   musl build, Docker build.

---

## 13. Build, packaging, integration

### 13.1 Static musl binary

```sh
docker run --rm -v "$PWD:/src" -w /src rust:alpine sh -c \
  'apk add musl-dev && cargo build --release --target x86_64-unknown-linux-musl'
# → target/x86_64-unknown-linux-musl/release/streagit  (fully static, ~6 MB stripped)
```

`rust:alpine` targets musl natively, so `cargo build --release` already
yields a static binary; the explicit target is for cross-building from the
Mac. `Cargo.toml` profile: `lto = "thin"`, `codegen-units = 1`,
`strip = true`, `panic = "abort"` is **not** used (panics in a render must
be caught per request: `spawn_blocking` returns `JoinError`, map to 500).

### 13.2 Dockerfile (streagit standalone)

```Dockerfile
FROM rust:alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY assets ./assets
RUN cargo build --release --locked
FROM alpine:3
COPY --from=build /src/target/release/streagit /usr/local/bin/streagit
USER 1000
EXPOSE 8081
ENTRYPOINT ["streagit"]
CMD ["--listen", "0.0.0.0:8081", "--root", "/data/data", "--prefix", "/browse"]
```

### 13.3 gitbackup integration (changes to gitbackup, to be done separately)

Goal: identical URLs, no pre-rendering. In gitbackup's Dockerfile replace
the `stagit` build stage by a `streagit` stage (or `COPY --from=
ghcr.io/marvambass/streagit:<tag>`), and:

1. **Spawn** streagit as a child process from gitbackup (`exec.Command`
   with `--listen 127.0.0.1:8081 --root <data> --prefix /browse
   --public-url $GITBACKUP_BROWSE_URL --cache-dir <state>/streagit
   --nice 10`), restart it on exit, or run both under a tiny init
   (`tini`/`s6`). Child process is simplest (one container, one image).
2. **Reverse-proxy** in `internal/web/web.go`: register
   `GET /browse/{path...}` so that paths whose segments contain
   `html`, `html.vN`, `html.<unit>[.vN]` or end in `.git/…` go to
   `httputil.NewSingleHostReverseProxy("http://127.0.0.1:8081")`
   (unchanged path), everything else to the existing `http.FileServer`.
   Simplest robust rule: proxy when the path matches
   `(^|/)(html(\.[A-Za-z0-9_~+-]+)*(\.v[0-9]+)?|[^/]+\.git)(/|$)`.
   Also proxy `/robots.txt`.
3. `internal/stagit`: `Build`/`BuildVersion` become no-ops (or are
   removed with the `stagit_dirty` plumbing); `Index()` keeps generating
   landing pages but links `html/log.html` etc. **unconditionally** when
   streagit is enabled (today it checks for `html/log.html` on disk:
   `HasHTML`, `version.HTML`). The `assets()` copies of
   `style.css`/`logo.png`/`favicon.png` into every html dir are no longer
   needed (streagit serves them); `data/style.css` + `favicon.png` for
   gitbackup's own index pages stay (or gitbackup passes
   `--assets-dir` so both use the same style).
4. Old `html/`, `html.vN/` directories: leave them (ignored, the proxy
   route wins) or delete them in a one-off reconcile step to reclaim
   disk (162 MB per ComfyUI-sized repo!).
5. Config: `settings.stagit: true` → keep the key, meaning "serve
   browsable pages"; plan-level `stagit.on_change/every` become obsolete.
6. Wiki pages come for free: link `html.wiki/log.html` on the landing page.

What can be dropped vs. stagit: the per-commit and per-file files, the
`.stagit-cache`, the full-rebuild-on-rewrite, the asset copies, the
`StagitDirty`/`StagitRebuild`/`StagitBuilt` scheduling, the 2 h timeout
per build. What stays for compatibility: the URL names, the HTML
structure/ids/classes, `style.css` selectors, relative-link convention
(`../` to the slot landing page), `description`/`url`/`owner` files,
`-u`-style absolute feed links.

---

## 14. Milestones

1. **M1 – read-only core** (end-to-end for gitbackup): resolve, repo
   cache, log page with `from=` paging + diffstat cache, commit page,
   files page (bytes), blob page, refs page, atom/tags, assets, 404/500,
   ETag/304, render cache, streaming with cancellation, limits (§6/§7/§9),
   config, Dockerfile, golden tests for the fixture, musl build.
2. **M2 – polish**: `?ref=` logs + linked refs, persistent stats cache,
   single-flight, `--files-line-counts`, robots, nice, HEAD fallbacks,
   `html.<unit>` wikis, compression, metrics endpoint (`/metrics`,
   Prometheus text: renders, cache hits, render seconds, queue drops).
3. **M3 – extras**: `raw/<path>`, `tree/<dir>/` pages, index page,
   `?page=K`, pre-compressed cache entries, `--stats-prune`, SHA-256 repos.

---

## 15. Open questions / things the implementer should verify

- gix API details change between versions; the names used here
  (`rev_walk().sorting(..)`, `tree.changes().options(|o| o.track_rewrites(..))
  .for_each_to_obtain_tree(..)`, `change.diff(&mut cache)?.line_counts()`,
  `gix::diff::blob::UnifiedDiff`, `find_header`) were compiled/run against
  gix 0.89.0 (the walk+line-count part, §3.2) or checked in its docs
  (`UnifiedDiff`, `find_header`, `lookup_entry_by_path`); re-check against
  the pinned version. Keep gix's default cargo features on (the hash
  algorithm is a feature; `default-features = false` fails to build).
- Whether gix's `line_counts()` treats "no newline at end of file"
  exactly like libgit2 (off-by-one in rare files) – the golden test on the
  fixture file without a trailing newline will tell.
- libgit2's `GIT_DIFF_FIND_COPIES` default copy source: libgit2 looks for
  copy sources among *all* files unless `GIT_DIFF_FIND_COPIES_FROM_UNMODIFIED`
  is set – i.e. only among modified files by default. gix's
  `CopySource::FromSetOfModifiedFiles` is the counterpart. Confirm with the
  fixture (copy of an unmodified file must show as `A`, copy of a modified
  file as `C`).
- Exact delta ordering for renames in libgit2 (by old path or new path)
  only matters for the `#hN` anchors; not important.
- The gitbackup reverse-proxy rule and `HasHTML` change (§13.3) are
  gitbackup's side; agree on the proxy rule before relying on
  `html.wiki`.
- NAS numbers: the 2.9 ms/commit figure is from an M-series laptop; measure
  on the NAS once M1 runs (`/metrics`).

---

## 16. License and attribution

streagit is MIT licensed. The HTML structure, the CSS (`assets/style.css`),
`logo.png` and `favicon.png` are taken from **stagit** by Hiltjo Posthuma,
MIT/X Consortium License, (c) 2015-2026 Hiltjo Posthuma
<hiltjo@codemadness.org>. The stagit copyright notice and permission
notice must be reproduced in streagit's `LICENSE` (second section) and
`README.md` must say that the page layout and stylesheet are derived from
stagit. No stagit C code is copied; the output format is re-implemented.

---

## 17. Implementation notes (v0.1.0)

Decisions taken while implementing against the real APIs, and where the code
knowingly differs from the text above. The golden test (`tests/golden_test.rs`)
compares every page of the fixture with stagit 1.3's output byte for byte, so
the items marked *fidelity* are facts about stagit/libgit2 that the sections
above got wrong or left open.

Fidelity (found by the golden test, now matched exactly):

- **"\ No newline at end of file"** (§4.5 item 6): libgit2 gives the marker the
  origin of the line it follows (after `+` → counted/rendered as an addition,
  after `-` → as a deletion, after a context line → context). streagit does the
  same, so counts match stagit exactly; the planned deviation is not needed.
- **Submodule entries are deltas** (§4.5 "submodules are ignored"): with
  `GIT_DIFF_IGNORE_SUBMODULES` libgit2 still reports gitlink additions /
  changes as deltas and diffs the synthetic content `Subproject commit <id>\n`
  (+1 line). streagit reproduces this.
- **Typechange deltas have no hunks**: libgit2 prints the `diff --git` line
  only, 0/0 in the diffstat (not a content diff of blob vs. symlink target).
- **Copies**: libgit2's exact-match copy source is the *old* side of a file
  modified in the same commit. gix's copy tracker matches the *new* side and
  drops the modification, so streagit uses gix for exact renames only and
  detects copies itself (`git::diff::detect_copies`).
- **Empty commit message**: `git_commit_summary` returns `""`, not NULL, so
  the log row still has an (empty) link and the atom entry an empty `<title>`;
  the `<content>` message block is printed unconditionally.
- **`<title>`**: the name is always printed; ` - ` precedes it only when the
  page title is non-empty (§4.3 implied the name was conditional).
- `<hr/>` is printed once before the first `diff --git`, not per delta.
- A final blob line without `\n` is written without one (`…</pre>`); `log.html`
  and `files.html` end with `</tbody></table>` directly followed by the footer.
- A tag pointing at a tree makes stagit stop iterating refs (§4.8); the
  fixture names it `zz-tree-tag` so that stagit still lists the real tags and
  the golden refs/tags pages are comparable. streagit skips such tags.

Design choices:

- Line diffs are computed directly with `gix::diff::blob` (imara-diff, Myers,
  `postprocess_no_heuristic`) on the two blob buffers rather than through gix's
  resource-cache platform: full control over context merging (gap ≤ 6 lines),
  xdiff's funcname rule (search upwards, bounded by the previous hunk's start,
  80 bytes), markers and size limits. Tree diffs use gix with exact rename
  tracking (`Rewrites { copies: None, percentage: None, limit: 1000 }`).
- gix 0.89 errors are `gix::Error` (gix-error); `is_not_found()` maps to 404.
  Repositories are opened with `open::Options::isolated()` (no user/system
  config, no environment).
- Cache keys and ETags include a hash of the canonical git dir: versions and
  forks share tips and trees with their parent, so keying by object id alone
  served the wrong repository's header.
- The persistent diffstat cache (§8.4) is one append-only file
  (`<cache-dir>/stats.v1.bin`, 32-byte records) for all repositories instead of
  one file per repo – object ids are globally unique. Blob line counts share
  the file (`files = u32::MAX` marks them). The tail is loaded when the file
  holds more than `--stats-entries` records; a torn record is dropped.
- Render permits: `try_acquire` first, otherwise wait if fewer than
  `--max-queue` requests are waiting, at most `--queue-timeout`, else 503.
  `HEAD` requests never render (headers only; `Content-Length` when cached).
- The render timeout check happens between rows/deltas/lines; the notice is
  appended to the already-sent page (200). If the queue wait used up the
  deadline before the first byte, 503 is returned instead of 504.
- Compression is gzip only (`tower-http`); ETags stay strong.
- `--allow-git-paths` requires `<name>.git` to match `[A-Za-z0-9_.~+-]+`;
  `html.<unit>` rejects units that look like versions (`v<digits>`).
- Build: `cargo-cache`/`streagit-target` Docker volumes keep the registry and
  target directory; the registry volume was reused from the §3.2 experiment.

Scope (§14): M1 complete; from M2 `?ref=`/`--link-refs`, persistent stats,
single-flight, `--files-line-counts`, robots, `--nice`, HEAD fallbacks,
`html.<unit>`, gzip and `/metrics` are done; from M3 `raw/<path>`. Not
implemented: `tree/<dir>/` pages, the index page (`--index`), `?page=K`,
pre-compressed cache entries, `--stats-prune`, a unix socket listener, header
read timeouts, `Range` requests, brotli. SHA-256 repositories: 64-hex ids are
accepted syntactically but untested.
