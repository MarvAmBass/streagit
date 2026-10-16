//! Commit diffs: tree diff with exact rename/copy detection, per-file line diffs with
//! unified hunks and libgit2-compatible counts (SPEC §4.5, §6.3, §7.2).

use gix::diff::blob::{Algorithm, Diff, InternedInput};
use gix::diff::Rewrites;
use gix::object::tree::diff::{Action, Change};
use gix::object::tree::EntryKind;
use gix::ObjectId;

use super::blob::is_binary;
use super::commit::CommitInfo;
use crate::render::out::RenderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaStatus {
    Added,
    Copied,
    Deleted,
    Modified,
    Renamed,
    TypeChange,
}

impl DeltaStatus {
    pub fn letter(self) -> char {
        match self {
            DeltaStatus::Added => 'A',
            DeltaStatus::Copied => 'C',
            DeltaStatus::Deleted => 'D',
            DeltaStatus::Modified => 'M',
            DeltaStatus::Renamed => 'R',
            DeltaStatus::TypeChange => 'T',
        }
    }
}

#[derive(Debug, Clone)]
pub struct Delta {
    pub status: DeltaStatus,
    pub old_path: Vec<u8>,
    pub new_path: Vec<u8>,
    pub old_id: Option<ObjectId>,
    pub new_id: Option<ObjectId>,
    pub old_kind: Option<EntryKind>,
    pub new_kind: Option<EntryKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

/// One diff line: a byte range into the new buffer (`Add`) or the old buffer (others).
/// A `marker` line is `\ No newline at end of file`; libgit2 gives it the kind of the
/// line it follows (stagit then renders and counts it like that line).
#[derive(Debug, Clone, Copy)]
pub struct Line {
    pub kind: LineKind,
    pub marker: bool,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    /// `@@ -a,b +c,d @@ funcname\n`
    pub header: Vec<u8>,
    pub lines: Vec<Line>,
}

#[derive(Debug)]
pub enum FileDiff {
    Binary,
    Skipped(String),
    Text {
        old: Vec<u8>,
        new: Vec<u8>,
        hunks: Vec<Hunk>,
    },
}

/// Per-delta counts (stagit's `addcount`/`delcount` contribution).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub adds: u32,
    pub dels: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub files: u32,
    pub adds: u32,
    pub dels: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    CountsOnly,
    Full,
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_diff_blob_bytes: u64,
    pub max_commit_diff_bytes: u64,
}

/// stagit's "Diff is too large, output suppressed." thresholds.
pub const MAX_DELTAS: usize = 1000;
pub const MAX_LINES: u32 = 100_000;

#[derive(Debug)]
pub struct CommitDiff {
    pub deltas: Vec<Delta>,
    /// Parallel to `deltas` (empty in `CountsOnly` mode or when too large).
    pub files: Vec<FileDiff>,
    pub counts: Vec<Counts>,
    pub stats: Stats,
    /// stagit would print "Diff is too large, output suppressed."
    pub too_large: bool,
}

/// Exact renames only; copies are detected afterwards (`detect_copies`) with libgit2's
/// semantics, which gix's copy tracking does not reproduce (it matches against the new
/// side of modified files and drops the modification).
pub fn rewrites() -> Rewrites {
    Rewrites {
        copies: None,
        percentage: None,
        limit: 1000,
        track_empty: false,
    }
}

/// libgit2 `GIT_DIFF_FIND_COPIES | GIT_DIFF_FIND_EXACT_MATCH_ONLY`: an addition whose blob
/// equals the *old* side of a file modified (or renamed) in the same commit is a copy of it.
pub fn detect_copies(deltas: &mut [Delta]) {
    let sources: Vec<(ObjectId, Vec<u8>, Option<EntryKind>)> = deltas
        .iter()
        .filter(|d| {
            matches!(
                d.status,
                DeltaStatus::Modified | DeltaStatus::TypeChange | DeltaStatus::Renamed
            )
        })
        .filter_map(|d| d.old_id.map(|id| (id, d.old_path.clone(), d.old_kind)))
        .collect();
    if sources.is_empty() {
        return;
    }
    for d in deltas.iter_mut() {
        if d.status != DeltaStatus::Added {
            continue;
        }
        let Some(new_id) = d.new_id else { continue };
        if let Some((id, path, kind)) = sources.iter().find(|(id, _, _)| *id == new_id) {
            d.status = DeltaStatus::Copied;
            d.old_id = Some(*id);
            d.old_path = path.clone();
            d.old_kind = *kind;
        }
    }
}

/// Entries that libgit2 reports as deltas: blobs, symlinks and submodule (gitlink) entries.
fn is_file(kind: EntryKind) -> bool {
    kind != EntryKind::Tree
}

/// libgit2's file "type" for typechange detection: regular file, symlink or submodule.
fn type_class(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Blob | EntryKind::BlobExecutable => 0,
        EntryKind::Link => 1,
        EntryKind::Commit => 2,
        EntryKind::Tree => 3,
    }
}

/// Tree diff of `commit` against its first parent (or the empty tree), sorted like libgit2.
pub fn tree_deltas(repo: &gix::Repository, commit: &CommitInfo) -> anyhow::Result<Vec<Delta>> {
    let new_tree = repo.find_tree(commit.tree)?;
    let old_tree = match commit.parent {
        Some(p) => repo.find_commit(p)?.tree()?,
        None => repo.empty_tree(),
    };
    let mut deltas: Vec<Delta> = Vec::new();
    let mut platform = old_tree.changes()?;
    platform.options(|o| {
        o.track_path().track_rewrites(Some(rewrites()));
    });
    let mut cache = repo.diff_resource_cache_for_tree_diff()?;
    platform.for_each_to_obtain_tree_with_cache(
        &new_tree,
        &mut cache,
        |change: Change<'_, '_, '_>| {
            let delta = match change {
                Change::Addition {
                    location,
                    entry_mode,
                    id,
                    ..
                } => {
                    if !is_file(entry_mode.kind()) {
                        return Ok(Action::Continue(()));
                    }
                    Delta {
                        status: DeltaStatus::Added,
                        old_path: location.to_vec(),
                        new_path: location.to_vec(),
                        old_id: None,
                        new_id: Some(id.detach()),
                        old_kind: None,
                        new_kind: Some(entry_mode.kind()),
                    }
                }
                Change::Deletion {
                    location,
                    entry_mode,
                    id,
                    ..
                } => {
                    if !is_file(entry_mode.kind()) {
                        return Ok(Action::Continue(()));
                    }
                    Delta {
                        status: DeltaStatus::Deleted,
                        old_path: location.to_vec(),
                        new_path: location.to_vec(),
                        old_id: Some(id.detach()),
                        new_id: None,
                        old_kind: Some(entry_mode.kind()),
                        new_kind: None,
                    }
                }
                Change::Modification {
                    location,
                    previous_entry_mode,
                    previous_id,
                    entry_mode,
                    id,
                } => {
                    let (ok, nk) = (previous_entry_mode.kind(), entry_mode.kind());
                    if !is_file(ok) || !is_file(nk) {
                        return Ok(Action::Continue(()));
                    }
                    let typechange = type_class(ok) != type_class(nk);
                    Delta {
                        status: if typechange {
                            DeltaStatus::TypeChange
                        } else {
                            DeltaStatus::Modified
                        },
                        old_path: location.to_vec(),
                        new_path: location.to_vec(),
                        old_id: Some(previous_id.detach()),
                        new_id: Some(id.detach()),
                        old_kind: Some(ok),
                        new_kind: Some(nk),
                    }
                }
                Change::Rewrite {
                    source_location,
                    source_entry_mode,
                    source_id,
                    entry_mode,
                    location,
                    id,
                    copy,
                    ..
                } => {
                    if !is_file(source_entry_mode.kind()) || !is_file(entry_mode.kind()) {
                        return Ok(Action::Continue(()));
                    }
                    Delta {
                        status: if copy {
                            DeltaStatus::Copied
                        } else {
                            DeltaStatus::Renamed
                        },
                        old_path: source_location.to_vec(),
                        new_path: location.to_vec(),
                        old_id: Some(source_id.detach()),
                        new_id: Some(id.detach()),
                        old_kind: Some(source_entry_mode.kind()),
                        new_kind: Some(entry_mode.kind()),
                    }
                }
            };
            deltas.push(delta);
            Ok(Action::Continue(()))
        },
    )?;
    detect_copies(&mut deltas);
    deltas.sort_by(|a, b| {
        let ka = if a.status == DeltaStatus::Deleted {
            &a.old_path
        } else {
            &a.new_path
        };
        let kb = if b.status == DeltaStatus::Deleted {
            &b.old_path
        } else {
            &b.new_path
        };
        ka.cmp(kb).then_with(|| a.old_path.cmp(&b.old_path))
    });
    Ok(deltas)
}

fn blob_size(repo: &gix::Repository, id: ObjectId) -> anyhow::Result<u64> {
    Ok(repo.find_header(id)?.size())
}

/// Blob content of one side; submodule entries get libgit2's synthetic content.
fn load(
    repo: &gix::Repository,
    id: Option<ObjectId>,
    kind: Option<EntryKind>,
) -> anyhow::Result<Vec<u8>> {
    match (id, kind) {
        (None, _) => Ok(Vec::new()),
        (Some(id), Some(EntryKind::Commit)) => Ok(format!("Subproject commit {id}\n").into_bytes()),
        (Some(id), _) => Ok(repo.find_object(id)?.data.clone()),
    }
}

/// Diff one commit. `check` is called between deltas (deadline / cancellation).
pub fn diff_commit(
    repo: &gix::Repository,
    commit: &CommitInfo,
    mode: Mode,
    limits: Limits,
    mut check: impl FnMut() -> Result<(), RenderError>,
) -> Result<CommitDiff, RenderError> {
    let deltas = tree_deltas(repo, commit).map_err(RenderError::Git)?;
    let mut too_large = deltas.len() > MAX_DELTAS;
    let keep = mode == Mode::Full && !too_large;
    let mut files: Vec<FileDiff> = Vec::with_capacity(if keep { deltas.len() } else { 0 });
    let mut counts: Vec<Counts> = Vec::with_capacity(deltas.len());
    let mut stats = Stats {
        files: deltas.len() as u32,
        adds: 0,
        dels: 0,
    };
    let mut retained: u64 = 0;

    for d in &deltas {
        check()?;
        // size limits per side
        let mut skip: Option<String> = None;
        for (id, kind) in [(d.old_id, d.old_kind), (d.new_id, d.new_kind)] {
            let Some(id) = id else { continue };
            if kind == Some(EntryKind::Commit) {
                continue;
            }
            let size = blob_size(repo, id).map_err(RenderError::Git)?;
            if size > limits.max_diff_blob_bytes {
                skip = Some(format!("Diff skipped, file too large ({size} bytes)."));
                break;
            }
        }
        if let Some(reason) = skip {
            counts.push(Counts::default());
            if keep {
                files.push(FileDiff::Skipped(reason));
            }
            continue;
        }
        let old = load(repo, d.old_id, d.old_kind).map_err(RenderError::Git)?;
        let new = load(repo, d.new_id, d.new_kind).map_err(RenderError::Git)?;
        if is_binary(&old) || is_binary(&new) {
            counts.push(Counts::default());
            if keep {
                files.push(FileDiff::Binary);
            }
            continue;
        }
        // identical blobs (mode change, pure rename/copy) have no hunks; libgit2 does
        // not diff the content of typechange deltas either
        let (hunks, c) = if d.old_id == d.new_id || d.status == DeltaStatus::TypeChange {
            (Vec::new(), Counts::default())
        } else {
            line_diff(&old, &new, keep)
        };
        stats.adds = stats.adds.saturating_add(c.adds);
        stats.dels = stats.dels.saturating_add(c.dels);
        counts.push(c);
        if keep {
            let bytes = (old.len() + new.len()) as u64;
            if retained + bytes > limits.max_commit_diff_bytes {
                files.push(FileDiff::Skipped(
                    "Diff skipped, commit too large.".to_string(),
                ));
            } else {
                retained += bytes;
                files.push(FileDiff::Text { old, new, hunks });
            }
        }
    }
    if stats.adds > MAX_LINES || stats.dels > MAX_LINES {
        too_large = true;
        files.clear();
    }
    Ok(CommitDiff {
        deltas,
        files,
        counts,
        stats,
        too_large,
    })
}

/// Byte offsets of line starts (len+1 entries).
fn line_offsets(data: &[u8]) -> Vec<usize> {
    let mut offs = Vec::with_capacity(data.len() / 32 + 2);
    offs.push(0);
    for (i, &c) in data.iter().enumerate() {
        if c == b'\n' {
            offs.push(i + 1);
        }
    }
    if *offs.last().unwrap() != data.len() {
        offs.push(data.len());
    }
    offs
}

/// xdiff's default funcname rule: a line starting with a letter, `_` or `$`,
/// cut to 80 bytes, trailing whitespace trimmed.
fn funcname(line: &[u8]) -> Option<&[u8]> {
    let first = *line.first()?;
    if !(first.is_ascii_alphabetic() || first == b'_' || first == b'$') {
        return None;
    }
    let mut len = line.len().min(80);
    while len > 0 && matches!(line[len - 1], b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C) {
        len -= 1;
    }
    Some(&line[..len])
}

const CTX: u32 = 3;

/// Myers line diff with unified hunks (3 lines of context). Returns the hunks (empty
/// when `keep` is false) and the libgit2-style counts.
pub fn line_diff(old: &[u8], new: &[u8], keep: bool) -> (Vec<Hunk>, Counts) {
    let input: InternedInput<&[u8]> = InternedInput::new(
        gix::diff::blob::sources::byte_lines(old),
        gix::diff::blob::sources::byte_lines(new),
    );
    let mut diff = Diff::compute(Algorithm::Myers, &input);
    diff.postprocess_no_heuristic(&input);
    let old_off = line_offsets(old);
    let new_off = line_offsets(new);
    let n_old = input.before.len() as u32;
    let n_new = input.after.len() as u32;
    let old_line = |i: u32| -> (usize, usize) { (old_off[i as usize], old_off[i as usize + 1]) };
    let new_line = |i: u32| -> (usize, usize) { (new_off[i as usize], new_off[i as usize + 1]) };

    let changes: Vec<gix::diff::blob::Hunk> = diff.hunks().collect();
    let mut hunks = Vec::new();
    let mut counts = Counts::default();
    let mut funclineprev: i64 = -1;
    let mut i = 0;
    while i < changes.len() {
        // group changes whose gap is <= 2*CTX
        let mut j = i;
        while j + 1 < changes.len()
            && changes[j + 1].before.start - changes[j].before.end <= 2 * CTX
        {
            j += 1;
        }
        let first = &changes[i];
        let last = &changes[j];
        let s1 = first.before.start.saturating_sub(CTX);
        let e1 = (last.before.end + CTX).min(n_old);
        let s2 = first.after.start.saturating_sub(CTX);
        let e2 = (last.after.end + CTX).min(n_new);

        let mut lines: Vec<Line> = Vec::new();
        let push = |kind: LineKind,
                    (start, end): (usize, usize),
                    data: &[u8],
                    lines: &mut Vec<Line>,
                    counts: &mut Counts| {
            match kind {
                LineKind::Add => counts.adds += 1,
                LineKind::Del => counts.dels += 1,
                _ => {}
            }
            if keep {
                lines.push(Line {
                    kind,
                    marker: false,
                    start,
                    end,
                });
            }
            if end > start && data[end - 1] != b'\n' {
                // the marker takes the kind of the line it follows (libgit2), and stagit
                // counts it accordingly
                match kind {
                    LineKind::Add => counts.adds += 1,
                    LineKind::Del => counts.dels += 1,
                    _ => {}
                }
                if keep {
                    lines.push(Line {
                        kind,
                        marker: true,
                        start: 0,
                        end: 0,
                    });
                }
            }
        };
        let mut o = s1;
        for ch in &changes[i..=j] {
            while o < ch.before.start {
                push(LineKind::Context, old_line(o), old, &mut lines, &mut counts);
                o += 1;
            }
            for k in ch.before.clone() {
                push(LineKind::Del, old_line(k), old, &mut lines, &mut counts);
            }
            for k in ch.after.clone() {
                push(LineKind::Add, new_line(k), new, &mut lines, &mut counts);
            }
            o = ch.before.end;
        }
        while o < e1 {
            push(LineKind::Context, old_line(o), old, &mut lines, &mut counts);
            o += 1;
        }

        if keep {
            let (c1, c2) = (e1 - s1, e2 - s2);
            let mut header = Vec::with_capacity(64);
            header.extend_from_slice(b"@@ -");
            header.extend_from_slice((if c1 != 0 { s1 + 1 } else { s1 }).to_string().as_bytes());
            if c1 != 1 {
                header.extend_from_slice(format!(",{c1}").as_bytes());
            }
            header.extend_from_slice(b" +");
            header.extend_from_slice((if c2 != 0 { s2 + 1 } else { s2 }).to_string().as_bytes());
            if c2 != 1 {
                header.extend_from_slice(format!(",{c2}").as_bytes());
            }
            header.extend_from_slice(b" @@");
            // function context: search upwards from the line above the hunk, not past the previous hunk
            let start = s1 as i64 - 1;
            let mut l = start;
            while l != funclineprev && l >= 0 && (l as u32) < n_old {
                let (a, b) = old_line(l as u32);
                if let Some(f) = funcname(&old[a..b]) {
                    if !f.is_empty() {
                        header.push(b' ');
                        header.extend_from_slice(f);
                    }
                    break;
                }
                l -= 1;
            }
            funclineprev = start;
            header.push(b'\n');
            hunks.push(Hunk { header, lines });
        }
        i = j + 1;
    }
    (hunks, counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(old: &[u8], new: &[u8]) -> (String, Counts) {
        let (hunks, c) = line_diff(old, new, true);
        let mut s = String::new();
        for h in &hunks {
            s.push_str(std::str::from_utf8(&h.header).unwrap());
            for l in &h.lines {
                if l.marker {
                    s.push_str(match l.kind {
                        LineKind::Add => "+",
                        LineKind::Del => "-",
                        LineKind::Context => " ",
                    });
                    s.push_str("\\ No newline at end of file\n");
                    continue;
                }
                match l.kind {
                    LineKind::Context => {
                        s.push(' ');
                        s.push_str(std::str::from_utf8(&old[l.start..l.end]).unwrap());
                    }
                    LineKind::Del => {
                        s.push('-');
                        s.push_str(std::str::from_utf8(&old[l.start..l.end]).unwrap());
                    }
                    LineKind::Add => {
                        s.push('+');
                        s.push_str(std::str::from_utf8(&new[l.start..l.end]).unwrap());
                    }
                }
            }
        }
        (s, c)
    }

    #[test]
    fn simple_modification() {
        let old = b"a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = b"a\nb\nc\nd\nX\nf\ng\nh\ni\nj\n";
        let (s, c) = render(old, new);
        assert_eq!(s, "@@ -2,7 +2,7 @@ a\n b\n c\n d\n-e\n+X\n f\n g\n h\n");
        assert_eq!(c, Counts { adds: 1, dels: 1 });
    }

    #[test]
    fn new_file_and_deleted_file() {
        let (s, c) = render(b"", b"one\ntwo\n");
        assert_eq!(s, "@@ -0,0 +1,2 @@\n+one\n+two\n");
        assert_eq!(c, Counts { adds: 2, dels: 0 });
        let (s, c) = render(b"one\n", b"");
        assert_eq!(s, "@@ -1 +0,0 @@\n-one\n");
        assert_eq!(c, Counts { adds: 0, dels: 1 });
    }

    #[test]
    fn no_newline_marker_follows_its_line() {
        let (s, c) = render(b"a\nb", b"a\nb\n");
        assert_eq!(
            s,
            "@@ -1,2 +1,2 @@\n a\n-b-\\ No newline at end of file\n+b\n"
        );
        assert_eq!(c, Counts { adds: 1, dels: 2 });
        let (s, c) = render(b"", b"x");
        assert_eq!(s, "@@ -0,0 +1 @@\n+x+\\ No newline at end of file\n");
        assert_eq!(c, Counts { adds: 2, dels: 0 });
        let (s, c) = render(b"a\nb", b"c\nb");
        assert_eq!(
            s,
            "@@ -1,2 +1,2 @@\n-a\n+c\n b \\ No newline at end of file\n"
        );
        assert_eq!(c, Counts { adds: 1, dels: 1 });
    }

    #[test]
    fn hunks_merge_within_six_lines_and_funcname() {
        let mut old = String::from("fn main() {\n");
        for i in 0..20 {
            old.push_str(&format!("    line{i}\n"));
        }
        let new = old
            .replace("line3\n", "LINE3\n")
            .replace("line10\n", "LINE10\n")
            .replace("line18\n", "LINE18\n");
        let (s, _) = render(old.as_bytes(), new.as_bytes());
        // changes at 4 and 11 are 6 apart -> merged; 18 is separate
        assert!(s.starts_with("@@ -2,14 +2,14 @@ fn main() {\n"), "{s}");
        // the second hunk's funcname search stops at the previous hunk start
        assert!(s.contains("@@ -17,5 +17,5 @@\n"), "{s}");
    }

    #[test]
    fn funcname_rule() {
        assert_eq!(funcname(b"def foo():   \n"), Some(&b"def foo():"[..]));
        assert_eq!(funcname(b"  indented\n"), None);
        assert_eq!(funcname(b"_x\n"), Some(&b"_x"[..]));
        let long = vec![b'a'; 100];
        assert_eq!(funcname(&long).unwrap().len(), 80);
    }
}
