//! Diffstat cache: commit id -> (files, +, -) and blob id -> line count (SPEC §8.3, §8.4).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gix::ObjectId;
use lru::LruCache;

use crate::git::diff::Stats;

const RECORD: usize = 32;
const FILE_NAME: &str = "stats.v1.bin";

/// Value stored per object id. Line counts of blobs use `files = u32::MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    files: u32,
    adds: u32,
    dels: u32,
}

const LINES_MARK: u32 = u32::MAX;

pub struct StatsCache {
    map: Mutex<LruCache<ObjectId, Entry>>,
    persist: Option<Mutex<File>>,
    pub path: Option<PathBuf>,
}

impl StatsCache {
    pub fn new(capacity: usize) -> StatsCache {
        StatsCache {
            map: Mutex::new(LruCache::new(NonZeroUsize::new(capacity.max(1)).unwrap())),
            persist: None,
            path: None,
        }
    }

    /// Load `<dir>/stats.v1.bin` (its tail if it holds more than `capacity` records)
    /// and append new records to it.
    pub fn with_dir(capacity: usize, dir: &Path) -> anyhow::Result<StatsCache> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(FILE_NAME);
        let mut cache = StatsCache::new(capacity);
        if let Ok(mut f) = File::open(&path) {
            let len = f.metadata().map(|m| m.len()).unwrap_or(0) as usize;
            let records = len / RECORD;
            let skip = records.saturating_sub(capacity);
            if skip > 0 {
                use std::io::Seek;
                f.seek(std::io::SeekFrom::Start((skip * RECORD) as u64))?;
            }
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            let mut map = cache.map.lock().unwrap_or_else(|e| e.into_inner());
            for rec in buf.chunks_exact(RECORD) {
                let id = ObjectId::from_bytes_or_panic(&rec[..20]);
                let u = |i: usize| u32::from_le_bytes([rec[i], rec[i + 1], rec[i + 2], rec[i + 3]]);
                map.put(
                    id,
                    Entry {
                        files: u(20),
                        adds: u(24),
                        dels: u(28),
                    },
                );
            }
            tracing::info!(records = buf.len() / RECORD, path = %path.display(), "loaded persistent diffstat cache");
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        cache.persist = Some(Mutex::new(file));
        cache.path = Some(path);
        Ok(cache)
    }

    fn put(&self, id: ObjectId, e: Entry) {
        let fresh = {
            let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
            map.put(id, e).is_none()
        };
        if fresh {
            if let Some(f) = &self.persist {
                let mut rec = [0u8; RECORD];
                let raw = id.as_bytes();
                rec[..raw.len().min(20)].copy_from_slice(&raw[..raw.len().min(20)]);
                rec[20..24].copy_from_slice(&e.files.to_le_bytes());
                rec[24..28].copy_from_slice(&e.adds.to_le_bytes());
                rec[28..32].copy_from_slice(&e.dels.to_le_bytes());
                let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(err) = f.write_all(&rec) {
                    tracing::warn!(%err, "writing diffstat cache");
                }
            }
        }
    }

    fn get(&self, id: &ObjectId) -> Option<Entry> {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.get(id).copied()
    }

    pub fn get_stats(&self, id: &ObjectId) -> Option<Stats> {
        self.get(id)
            .filter(|e| e.files != LINES_MARK)
            .map(|e| Stats {
                files: e.files,
                adds: e.adds,
                dels: e.dels,
            })
    }

    pub fn put_stats(&self, id: ObjectId, s: Stats) {
        self.put(
            id,
            Entry {
                files: s.files.min(LINES_MARK - 1),
                adds: s.adds,
                dels: s.dels,
            },
        );
    }

    pub fn get_lines(&self, id: &ObjectId) -> Option<u32> {
        self.get(id)
            .filter(|e| e.files == LINES_MARK)
            .map(|e| e.adds)
    }

    pub fn put_lines(&self, id: ObjectId, lines: u32) {
        self.put(
            id,
            Entry {
                files: LINES_MARK,
                adds: lines,
                dels: 0,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let id = ObjectId::from_hex(b"0123456789abcdef0123456789abcdef01234567").unwrap();
        let blob = ObjectId::from_hex(b"abcdef0123456789abcdef0123456789abcdef01").unwrap();
        {
            let c = StatsCache::with_dir(10, dir.path()).unwrap();
            c.put_stats(
                id,
                Stats {
                    files: 3,
                    adds: 10,
                    dels: 2,
                },
            );
            c.put_lines(blob, 42);
            assert_eq!(
                c.get_stats(&id),
                Some(Stats {
                    files: 3,
                    adds: 10,
                    dels: 2
                })
            );
            assert_eq!(c.get_lines(&blob), Some(42));
            assert_eq!(c.get_lines(&id), None);
        }
        let c = StatsCache::with_dir(10, dir.path()).unwrap();
        assert_eq!(
            c.get_stats(&id),
            Some(Stats {
                files: 3,
                adds: 10,
                dels: 2
            })
        );
        assert_eq!(c.get_lines(&blob), Some(42));
        // a torn record at the end is ignored
        let path = dir.path().join(FILE_NAME);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&[1, 2, 3])
            .unwrap();
        let c = StatsCache::with_dir(10, dir.path()).unwrap();
        assert_eq!(c.len(), 2);
    }
}
