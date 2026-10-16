//! Bytes-bounded LRU of rendered pages (SPEC §8.2).

use std::num::NonZeroUsize;
use std::sync::Mutex;

use bytes::Bytes;
use lru::LruCache;

pub struct RenderCache {
    inner: Mutex<Inner>,
    max_bytes: usize,
    pub max_entry: usize,
}

struct Inner {
    map: LruCache<String, Bytes>,
    bytes: usize,
    hits: u64,
    misses: u64,
}

impl RenderCache {
    pub fn new(max_bytes: usize, max_entries: usize, max_entry: usize) -> RenderCache {
        RenderCache {
            inner: Mutex::new(Inner {
                map: LruCache::new(NonZeroUsize::new(max_entries.max(1)).unwrap()),
                bytes: 0,
                hits: 0,
                misses: 0,
            }),
            max_bytes,
            max_entry: max_entry.min(max_bytes),
        }
    }

    pub fn get(&self, key: &str) -> Option<Bytes> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let r = g.map.get(key).cloned();
        if r.is_some() {
            g.hits += 1;
        } else {
            g.misses += 1;
        }
        r
    }

    /// Store a page unless it is larger than `max_entry`. Returns whether it was stored.
    pub fn put(&self, key: String, body: Bytes) -> bool {
        if body.len() > self.max_entry {
            return false;
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = g.map.pop(&key) {
            g.bytes -= old.len();
        }
        g.bytes += body.len();
        g.map.put(key, body);
        while g.bytes > self.max_bytes {
            match g.map.pop_lru() {
                Some((_, v)) => g.bytes -= v.len(),
                None => break,
            }
        }
        true
    }

    pub fn stats(&self) -> (usize, usize, u64, u64) {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (g.map.len(), g.bytes, g.hits, g.misses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_by_bytes() {
        let c = RenderCache::new(100, 1000, 60);
        assert!(c.put("a".into(), Bytes::from(vec![0u8; 50])));
        assert!(c.put("b".into(), Bytes::from(vec![0u8; 50])));
        assert!(c.get("a").is_some());
        // c evicts the least recently used (b)
        assert!(c.put("c".into(), Bytes::from(vec![0u8; 50])));
        assert!(c.get("b").is_none());
        assert!(c.get("a").is_some());
        assert!(!c.put("big".into(), Bytes::from(vec![0u8; 61])));
        let (n, bytes, _, _) = c.stats();
        assert_eq!((n, bytes), (2, 100));
    }
}
