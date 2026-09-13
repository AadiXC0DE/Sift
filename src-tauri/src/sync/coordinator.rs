//! Account sync coordinator (P4.3).
//!
//! Every trigger that wants a sync - the poll timer, IDLE push, a manual
//! refresh, or the post-outbox refresh - goes through one coordinator per
//! account. It owns one cancellation token (tied to the account's generation)
//! and one tick slot, so:
//!
//! * one full/partial/reconcile tick runs at a time;
//! * a trigger that arrives while a tick runs is merged into a single pending
//!   flag, and the running tick performs at most ONE coalesced follow-up;
//! * a trigger that arrives while nothing runs runs immediately.
//!
//! Lock order (single, never violated): coordinator tick slot → IMAP worker
//! connection lease. Nothing inside a connection lease ever acquires a
//! coordinator slot, so the two can never deadlock.
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct SyncCoordinator {
    /// The tick slot. Held for the whole tick, including the follow-up pass.
    tick: Mutex<()>,
    /// A trigger arrived while a tick was running.
    pending: AtomicBool,
    /// The account generation's token; a cancelled coordinator stops waiting
    /// for a slot and never starts another pass.
    cancel: CancellationToken,
    /// Completed passes (observability + tests).
    runs: AtomicUsize,
}

impl SyncCoordinator {
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            tick: Mutex::new(()),
            pending: AtomicBool::new(false),
            cancel,
            runs: AtomicUsize::new(0),
        }
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Completed passes so far (1 or 2 per owning call).
    pub fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }

    /// True while another task holds the tick slot.
    pub fn is_ticking(&self) -> bool {
        self.tick.try_lock().is_err()
    }

    /// Ask for a tick and run it when this caller owns the slot.
    ///
    /// Returns the number of passes it ran: 0 when an in-flight tick
    /// coalesced this request, 1 normally, 2 when a trigger landed while the
    /// first pass ran (the documented "at most one coalesced tick").
    ///
    /// The request is recorded *before* the slot is probed, so a tick that is
    /// just finishing cannot drop it: either we take the slot and run, or the
    /// running tick sees the flag and performs the follow-up pass.
    pub async fn tick_now<F, Fut>(&self, job: F) -> usize
    where
        F: Fn() -> Fut,
        Fut: Future<Output = ()>,
    {
        if self.cancel.is_cancelled() {
            return 0;
        }
        self.pending.store(true, Ordering::SeqCst);
        let Ok(_guard) = self.tick.try_lock() else {
            // A tick owns the slot; it will run the coalesced pass.
            return 0;
        };
        // This pass is the request that made us run.
        self.pending.store(false, Ordering::SeqCst);
        job().await;
        let mut runs = 1;
        if !self.cancel.is_cancelled() && self.pending.swap(false, Ordering::SeqCst) {
            job().await;
            runs = 2;
        }
        self.runs.fetch_add(runs, Ordering::SeqCst);
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn p43_coordinator_serializes_and_coalesces() {
        let runs = Arc::new(AtomicUsize::new(0));
        let coord = Arc::new(SyncCoordinator::new(CancellationToken::new()));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let job = {
            let (runs, entered, release) = (runs.clone(), entered.clone(), release.clone());
            move || {
                let (runs, entered, release) = (runs.clone(), entered.clone(), release.clone());
                async move {
                    // Only the FIRST pass parks; the coalesced pass runs straight
                    // through (otherwise the test would deadlock by design).
                    let first = runs.fetch_add(1, Ordering::SeqCst) == 0;
                    if first {
                        entered.notify_waiters();
                        release.notified().await;
                    }
                }
            }
        };
        // First tick owns the slot and blocks inside the job.
        let first = {
            let (coord, job) = (coord.clone(), job.clone());
            tokio::spawn(async move { coord.tick_now(job).await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .expect("first tick entered");
        // Three overlapping triggers: two would run concurrently without the
        // coordinator; here they must collapse into one pending flag.
        for _ in 0..3 {
            let (coord, job) = (coord.clone(), job.clone());
            let handle = tokio::spawn(async move { coord.tick_now(job).await });
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_millis(200), handle)
                    .await
                    .expect("a coalesced request returns immediately")
                    .unwrap(),
                0
            );
        }
        release.notify_waiters();
        let owned = first.await.unwrap();
        assert_eq!(owned, 2, "one tick plus exactly one coalesced tick");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert_eq!(coord.runs(), 2);
    }

    #[tokio::test]
    async fn p43_coordinator_cancelled_never_runs() {
        let token = CancellationToken::new();
        let coord = SyncCoordinator::new(token.clone());
        token.cancel();
        let runs = Arc::new(AtomicUsize::new(0));
        let r = runs.clone();
        let n = coord
            .tick_now(move || {
                let r = r.clone();
                async move {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
            .await;
        assert_eq!(n, 0);
        assert_eq!(runs.load(Ordering::SeqCst), 0);
    }
}
