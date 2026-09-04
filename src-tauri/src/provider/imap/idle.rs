//! IDLE push (Phase 11 task 9): one connection sits in `IDLE` on INBOX;
//! any untagged `EXISTS`/`EXPUNGE`/`FETCH` becomes a `WatchEvent::Changed`
//! and the engine runs a partial sync (debounced 1 s in runtime).
//!
//! Gmail drops idle connections at ~30 min, so IDLE is re-issued every
//! 25 min. On error the task backs off and retries on its own cadence while
//! polling covers the gap; auth-terminal failures end the stream and the
//! runtime re-arms after the account re-authenticates.
use super::{
    conn::ImapPool,
    proto::{Response, Untagged},
};
use crate::errors::SiftError;
use crate::provider::WatchEvent;
use futures::channel::mpsc::UnboundedSender;
use std::time::Duration;

pub async fn idle_loop(
    pool: ImapPool,
    tx: UnboundedSender<WatchEvent>,
    idle_timeout: Duration,
    retry_base: Duration,
) {
    let mut backoff = retry_base;
    loop {
        if tx.is_closed() {
            return;
        }
        match idle_once(&pool, &tx, idle_timeout).await {
            Ok(()) => {
                backoff = retry_base;
            }
            Err(e) => {
                if is_terminal(&e) {
                    log::debug!(target: "sift::imap", "IDLE ending (terminal): {e}");
                    return;
                }
                log::debug!(target: "sift::imap", "IDLE error ({e}); retry in {backoff:?}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(300));
            }
        }
    }
}

fn is_terminal(e: &SiftError) -> bool {
    matches!(e, SiftError::App { code, .. } if matches!(
        code.as_str(),
        "imap_bad_password"
            | "imap_needs_app_password"
            | "imap_web_login_required"
            | "imap_disabled_by_admin"
            | "imap_tls"
    ))
}

fn is_data(r: &Response) -> bool {
    matches!(
        r,
        Response::Untagged(Untagged::Exists(_))
            | Response::Untagged(Untagged::Expunge(_))
            | Response::Untagged(Untagged::Fetch { .. })
    )
}

/// One IDLE session: SELECT INBOX, IDLE until an event or the timeout, DONE.
/// Returns normally in all cases; errors propagate for backoff handling.
async fn idle_once(
    pool: &ImapPool,
    tx: &UnboundedSender<WatchEvent>,
    idle_timeout: Duration,
) -> Result<(), SiftError> {
    // Reuse the parked connection when still alive, else connect fresh.
    let mut conn = match pool.take_idle().await {
        Some(c) => c,
        None => pool.fresh_conn().await?,
    };
    let session = async {
        conn.select("INBOX", false).await?;
        conn.start_idle().await?;
        let deadline = tokio::time::sleep(idle_timeout);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                res = conn.read_response() => {
                    let r = res?;
                    if is_data(&r) {
                        let _ = tx.unbounded_send(WatchEvent::Changed);
                    } else if !matches!(r, Response::Untagged(_)) {
                        // Tagged/server BYE ends the session.
                        break;
                    }
                }
            }
            if tx.is_closed() {
                break;
            }
        }
        // End IDLE, folding any trailing untagged into one more event.
        conn.write_raw(b"DONE\r\n").await?;
        let mut changed = false;
        loop {
            match conn.read_response().await? {
                r if is_data(&r) => changed = true,
                // Any tagged completion ends this IDLE session (ours is
                // the only command in flight on this connection).
                Response::TaggedOk { .. }
                | Response::TaggedNo { .. }
                | Response::TaggedBad { .. } => break,
                _ => {}
            }
        }
        if changed {
            let _ = tx.unbounded_send(WatchEvent::Changed);
        }
        Ok::<(), SiftError>(())
    };
    let outcome = session.await;
    // Park live connections for the next cycle; drop dead ones.
    match outcome {
        Ok(()) => pool.put_idle(Some(conn)).await,
        Err(e) => {
            pool.put_idle(None).await;
            return Err(e);
        }
    }
    Ok(())
}
