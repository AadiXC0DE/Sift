use crate::provider::{store_parsed, SyncSink};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub struct BackfillGate {
    pub foreground: Arc<AtomicUsize>,
}
impl Default for BackfillGate {
    fn default() -> Self {
        Self::new()
    }
}

impl BackfillGate {
    pub fn new() -> Self {
        Self {
            foreground: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub fn enter(&self) -> FG {
        self.foreground.fetch_add(1, Ordering::SeqCst);
        FG { g: self.clone() }
    }
}
pub struct FG {
    g: BackfillGate,
}
impl Drop for FG {
    fn drop(&mut self) {
        self.g.foreground.fetch_sub(1, Ordering::SeqCst);
    }
}

pub async fn run_backfill_once(
    sink: &dyn SyncSink,
    account_id: &str,
    provider: &dyn crate::provider::Provider,
    gate: &BackfillGate,
    horizon_days: i64,
) -> anyhow::Result<usize> {
    let min_date = crate::db::now_ms() - horizon_days * 24 * 3600 * 1000;
    let ids = sink.next_bodies_to_fetch(account_id, 20, min_date).await?;
    if ids.is_empty() {
        return Ok(0);
    }
    if gate.foreground.load(Ordering::SeqCst) > 0 {
        return Ok(0);
    } // yield to foreground
    let mut n = 0;
    for id in ids {
        if gate.foreground.load(Ordering::SeqCst) > 0 {
            break;
        }
        match provider.fetch_body(&id).await {
            Ok(parsed) => {
                if store_parsed(sink, &id, &parsed).await.is_ok() {
                    n += 1;
                }
            }
            Err(_) => {
                // mark error to avoid hot loop? keep none so it retries later with backoff
                break;
            }
        }
    }
    Ok(n)
}
