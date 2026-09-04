//! P11-T09: IDLE push, re-issue interval, reconnect after drop.
#[path = "support/mod.rs"]
mod support;

use futures::StreamExt;
use std::time::Duration;

fn pool_for(port: u16) -> sift::provider::imap::conn::ImapPool {
    sift::provider::imap::conn::ImapPool::new(
        "user@gmail.com".into(),
        "goodpassword0000".into(),
        "127.0.0.1".into(),
        port,
        true,
    )
}

fn idle_count(fake: &support::fake_imap::FakeGmail) -> usize {
    fake.state
        .lock()
        .unwrap()
        .commands_seen
        .iter()
        .filter(|c| c.starts_with("IDLE"))
        .count()
}

#[tokio::test]
async fn p11_t09_idle_push_reissue_reconnect() {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = pool_for(fake.addr.port());
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let h = tokio::spawn(sift::provider::imap::idle::idle_loop(
        pool,
        tx,
        Duration::from_secs(2),
        Duration::from_millis(200),
    ));
    // Let IDLE establish, then deliver: event within 1.5 s.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(idle_count(&fake) >= 1, "IDLE must be issued");
    fake.deliver(
        "all",
        "Zoe",
        "Knock knock",
        vec!["INBOX".into()],
        true,
    );
    let evt = tokio::time::timeout(Duration::from_millis(1500), rx.next())
        .await
        .expect("event within 1.5 s")
        .expect("stream alive");
    assert_eq!(evt, sift::provider::WatchEvent::Changed);

    // Re-issued after the (short test) interval.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        idle_count(&fake) >= 2,
        "IDLE re-issued, got {}",
        idle_count(&fake)
    );

    // Drop the connection mid-IDLE: task reconnects and IDLEs again.
    let before = idle_count(&fake);
    {
        let n = fake.state.lock().unwrap().commands_seen.len();
        fake.state.lock().unwrap().behavior.drop_after_n = Some(n + 1);
    }
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        idle_count(&fake) > before,
        "IDLE restored after drop ({} -> {})",
        before,
        idle_count(&fake)
    );
    h.abort();
}
