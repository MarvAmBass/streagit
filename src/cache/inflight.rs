//! Single-flight: concurrent requests for the same cache key share one render (SPEC §9).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tokio::sync::watch;

/// `None` = still rendering; `Some(None)` = finished but not cacheable; `Some(Some(b))` = page.
type Slot = Option<Option<Bytes>>;

#[derive(Default)]
pub struct InFlight {
    map: Mutex<HashMap<String, watch::Receiver<Slot>>>,
}

pub enum Ticket {
    /// This request renders; it must call `Leader::finish` (or drop it) when done.
    Leader(Leader),
    /// Another request is rendering the same page; await the result.
    Follower(watch::Receiver<Slot>),
}

pub struct Leader {
    key: String,
    tx: watch::Sender<Slot>,
    map: Arc<InFlight>,
    done: bool,
}

impl InFlight {
    pub fn begin(self: &Arc<Self>, key: &str) -> Ticket {
        let mut g = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(rx) = g.get(key) {
            return Ticket::Follower(rx.clone());
        }
        let (tx, rx) = watch::channel(None);
        g.insert(key.to_string(), rx);
        Ticket::Leader(Leader {
            key: key.to_string(),
            tx,
            map: self.clone(),
            done: false,
        })
    }

    fn remove(&self, key: &str) {
        let mut g = self.map.lock().unwrap_or_else(|e| e.into_inner());
        g.remove(key);
    }
}

impl Leader {
    pub fn finish(mut self, result: Option<Bytes>) {
        self.done = true;
        self.map.remove(&self.key);
        let _ = self.tx.send(Some(result));
    }
}

impl Drop for Leader {
    fn drop(&mut self) {
        if !self.done {
            self.map.remove(&self.key);
            let _ = self.tx.send(Some(None));
        }
    }
}

/// Wait for the leader's result.
pub async fn follow(mut rx: watch::Receiver<Slot>) -> Option<Bytes> {
    match rx.wait_for(|v| v.is_some()).await {
        Ok(v) => v.clone().flatten(),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn followers_get_the_leaders_result() {
        let f = Arc::new(InFlight::default());
        let Ticket::Leader(leader) = f.begin("k") else {
            panic!("first is leader")
        };
        let Ticket::Follower(rx) = f.begin("k") else {
            panic!("second follows")
        };
        let waiter = tokio::spawn(follow(rx));
        leader.finish(Some(Bytes::from_static(b"page")));
        assert_eq!(waiter.await.unwrap(), Some(Bytes::from_static(b"page")));
        // after finishing, the next request leads again
        assert!(matches!(f.begin("k"), Ticket::Leader(_)));
    }

    #[tokio::test]
    async fn dropped_leader_releases_followers() {
        let f = Arc::new(InFlight::default());
        let Ticket::Leader(leader) = f.begin("k") else {
            panic!()
        };
        let Ticket::Follower(rx) = f.begin("k") else {
            panic!()
        };
        drop(leader);
        assert_eq!(follow(rx).await, None);
    }
}
