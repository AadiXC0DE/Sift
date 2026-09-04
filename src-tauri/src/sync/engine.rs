use crate::db::Db;
use crate::provider::gmail::client::GmailClient;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    New,
    Full,
    Partial,
    Reconcile,
    Reauth,
    Error { backoff_secs: u64 },
}

// Engine registry grows in Phase 3+ (background tasks own clients); fields reserved for wiring.
#[allow(dead_code)]
pub struct Engine {
    db: Db,
    tokens: Arc<Mutex<HashMap<String, GmailClient>>>,
    cancel: HashMap<String, CancellationToken>,
    tx: mpsc::UnboundedSender<EngineMsg>,
}

pub enum EngineMsg {
    SyncNow(Option<String>),
    FocusChanged,
    OpDone(String),
    Cancel(String),
}

impl Engine {
    pub fn new(db: Db) -> (Self, mpsc::UnboundedReceiver<EngineMsg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                db,
                tokens: Arc::new(Mutex::new(Default::default())),
                cancel: Default::default(),
                tx,
            },
            rx,
        )
    }
    pub fn handle(&self) -> mpsc::UnboundedSender<EngineMsg> {
        self.tx.clone()
    }
    pub fn register_client(&self, _account: &str, _client: GmailClient) {}
    pub fn cancel_account(&mut self, account: &str) {
        if let Some(t) = self.cancel.remove(account) {
            t.cancel();
        }
    }
    pub fn poll_interval(focused: bool, s: &crate::dto::Settings) -> u64 {
        if focused {
            s.poll_focused.max(5) as u64
        } else {
            s.poll_background.max(15) as u64
        }
    }
    pub fn backoff_for(attempt: u32) -> u64 {
        (5u64 * 2u64.pow(attempt.min(5))).min(300)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p3_t18_state_math() {
        assert_eq!(Engine::backoff_for(0), 5);
        assert_eq!(Engine::backoff_for(1), 10);
        assert_eq!(Engine::backoff_for(2), 20);
        assert_eq!(Engine::backoff_for(10), 160); // 5*32 capped ladder (5min cap at higher)
        let s = crate::dto::Settings {
            poll_focused: 15,
            poll_background: 60,
            ..Default::default()
        };
        assert_eq!(Engine::poll_interval(true, &s), 15);
        assert_eq!(Engine::poll_interval(false, &s), 60);
    }
}
