//! Chunked output sink feeding an HTTP body through a bounded channel (SPEC §7.1).

use std::time::Instant;

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
    /// A chunk that cannot be handed over by then (the client is not reading)
    /// aborts the render: a render permit is never held longer than the deadline.
    deadline: Instant,
}

impl ChunkOut {
    pub fn new(tx: mpsc::Sender<Bytes>, max_keep: usize, deadline: Instant) -> ChunkOut {
        ChunkOut {
            buf: BytesMut::with_capacity(CHUNK),
            tx,
            keep: Some(Vec::new()),
            max_keep,
            total: 0,
            deadline,
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

    /// Hand a chunk to the body stream, waiting at most until the deadline.
    fn send(&mut self, chunk: Bytes) -> R {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let until = tokio::time::Instant::from_std(self.deadline);
                match handle.block_on(tokio::time::timeout_at(until, self.tx.send(chunk))) {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(_)) => Err(RenderError::Cancelled),
                    Err(_) => Err(RenderError::SlowClient),
                }
            }
            // outside a runtime (unit tests): plain blocking send
            Err(_) => self
                .tx
                .blocking_send(chunk)
                .map_err(|_| RenderError::Cancelled),
        }
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
        self.send(chunk)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_client_that_does_not_read_cannot_hold_the_render_past_the_deadline() {
        let (tx, _rx) = mpsc::channel::<Bytes>(1);
        let deadline = Instant::now() + Duration::from_millis(200);
        let started = Instant::now();
        let r = tokio::task::spawn_blocking(move || {
            let mut out = ChunkOut::new(tx, 1 << 20, deadline);
            // first chunk fills the channel, the second cannot be delivered
            out.write(&vec![b'x'; CHUNK])?;
            out.write(&vec![b'y'; CHUNK])
        })
        .await
        .unwrap();
        assert!(matches!(r, Err(RenderError::SlowClient)), "{r:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
