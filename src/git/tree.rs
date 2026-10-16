//! Tree listing and path lookup (SPEC §4.6, §6.4, §6.5).

use gix::object::tree::EntryKind;
use gix::ObjectId;

#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Full path from the root of the tree.
    pub path: Vec<u8>,
    pub mode: u16,
    pub kind: EntryKind,
    pub id: ObjectId,
}

/// stagit's `filemode()`: a type char followed by the rwx bits of the octal mode.
pub fn mode_string(mode: u16, kind: EntryKind) -> String {
    let mut s = String::with_capacity(10);
    s.push(match kind {
        EntryKind::Tree => 'd',
        EntryKind::Link => 'l',
        EntryKind::Commit => 'm',
        EntryKind::Blob | EntryKind::BlobExecutable => '-',
    });
    let m = mode as u32;
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, c) in bits {
        s.push(if m & bit != 0 { c } else { '-' });
    }
    s
}

/// Recursive, depth-first listing in tree order; directories are not listed.
/// Stops after `max` entries and reports whether more were available.
pub fn list_files(
    repo: &gix::Repository,
    tree_id: ObjectId,
    max: usize,
    mut check: impl FnMut() -> Result<(), crate::render::out::RenderError>,
) -> Result<(Vec<FileEntry>, bool), crate::render::out::RenderError> {
    let mut out = Vec::new();
    let mut truncated = false;
    let mut prefix: Vec<u8> = Vec::new();
    walk(
        repo,
        tree_id,
        &mut prefix,
        max,
        &mut out,
        &mut truncated,
        &mut check,
    )?;
    Ok((out, truncated))
}

fn walk(
    repo: &gix::Repository,
    tree_id: ObjectId,
    prefix: &mut Vec<u8>,
    max: usize,
    out: &mut Vec<FileEntry>,
    truncated: &mut bool,
    check: &mut impl FnMut() -> Result<(), crate::render::out::RenderError>,
) -> Result<(), crate::render::out::RenderError> {
    check()?;
    let tree = repo.find_tree(tree_id).map_err(anyhow::Error::from)?;
    let mut entries = Vec::new();
    for e in tree.iter() {
        let e = e.map_err(anyhow::Error::from)?;
        entries.push((e.filename().to_vec(), e.mode(), e.object_id()));
    }
    for (name, mode, id) in entries {
        let base = prefix.len();
        prefix.extend_from_slice(&name);
        if mode.is_tree() {
            prefix.push(b'/');
            walk(repo, id, prefix, max, out, truncated, check)?;
        } else {
            if out.len() >= max {
                *truncated = true;
                prefix.truncate(base);
                return Ok(());
            }
            out.push(FileEntry {
                path: prefix.clone(),
                mode: mode.value(),
                kind: mode.kind(),
                id,
            });
        }
        prefix.truncate(base);
        if *truncated {
            return Ok(());
        }
    }
    Ok(())
}

/// Look up `segments` (already split on `/`) in the tree. Returns the entry if it is
/// a blob or symlink; `None` for missing paths, directories and submodules.
pub fn lookup_blob(
    repo: &gix::Repository,
    tree_id: ObjectId,
    segments: &[&[u8]],
) -> anyhow::Result<Option<FileEntry>> {
    if segments.is_empty() {
        return Ok(None);
    }
    let tree = repo.find_tree(tree_id)?;
    let entry = tree.lookup_entry(segments.iter().copied())?;
    Ok(entry.and_then(|e| {
        let mode = e.mode();
        if mode.is_blob_or_symlink() {
            Some(FileEntry {
                path: segments.join(&b'/'),
                mode: mode.value(),
                kind: mode.kind(),
                id: e.object_id(),
            })
        } else {
            None
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert_eq!(mode_string(0o100644, EntryKind::Blob), "-rw-r--r--");
        assert_eq!(
            mode_string(0o100755, EntryKind::BlobExecutable),
            "-rwxr-xr-x"
        );
        assert_eq!(mode_string(0o120000, EntryKind::Link), "l---------");
        assert_eq!(mode_string(0o160000, EntryKind::Commit), "m---------");
        assert_eq!(mode_string(0o040000, EntryKind::Tree), "d---------");
    }
}
