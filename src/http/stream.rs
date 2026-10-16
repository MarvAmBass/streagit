//! Chunked output sink feeding an HTTP body through a bounded channel (SPEC §7.1).

use bytes::{Bytes, BytesMut};
use tokio::sync::mpsc;

use crate::render::out::{Out, RenderError, R};

pub const CHUNK: usize = 64 * 1024;
pub const CHANNEL_DEPTH: usize = 8;

pub struct ChunkOut {
    buf: BytesMut,
    tx: mpsc::Sender<Bytes>,
    /// Copy of everything written, kept while it fits the render cache.
    keep: Option<Vec<u8>>,
    max_keep: usize,
    total: usize,
}

impl ChunkOut {
    pub fn new(tx: mpsc::Sender<Bytes>, max_keep: usize) -> ChunkOut {
        ChunkOut {
            buf: BytesMut::with_capacity(CHUNK),
            tx,
            keep: Some(Vec::new()),
            max_keep,
            total: 0,
        }
    }

    pub fn total(&self) -> usize {
        self.total
    }

    /// Flush what is left and hand back the full page if it was small enough to cache.
    pub fn finish(mut self) -> Result<Option<Bytes>, RenderError> {
        self.flush()?;
        Ok(self.keep.take().map(Bytes::from))
    }

    /// Give up on caching (partial / failed page) and drop the copy.
    pub fn uncacheable(&mut self) {
        self.keep = None;
    }
}

impl Out for ChunkOut {
    fn write(&mut self, b: &[u8]) -> R {
        self.buf.extend_from_slice(b);
        if self.buf.len() >= CHUNK {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> R {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = self.buf.split().freeze();
        self.total += chunk.len();
        if let Some(k) = &mut self.keep {
            if k.len() + chunk.len() <= self.max_keep {
                k.extend_from_slice(&chunk);
            } else {
                self.keep = None;
            }
        }
        self.tx
            .blocking_send(chunk)
            .map_err(|_| RenderError::Cancelled)
    }
}

/// Byte sink that drops everything (used for HEAD requests that must still validate).
pub struct NullOut;

impl Out for NullOut {
    fn write(&mut self, _: &[u8]) -> R {
        Ok(())
    }
    fn flush(&mut self) -> R {
        Ok(())
    }
}
