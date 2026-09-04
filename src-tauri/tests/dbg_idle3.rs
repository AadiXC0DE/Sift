#[path = "support/mod.rs"]
mod support;
use std::time::Duration;

#[tokio::test]
async fn dbg_drop() {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = sift::provider::imap::conn::ImapPool::new(
        "u@g.com".into(), "goodpassword0000".into(),
        "127.0.0.1".into(), fake.addr.port(), true,
    );
    let (tx, _rx) = futures::channel::mpsc::unbounded();
    let h = tokio::spawn(sift::provider::imap::idle::idle_loop(
        pool, tx, Duration::from_secs(2), Duration::from_millis(200),
    ));
    tokio::time::sleep(Duration::from_millis(800)).await;
    let n = fake.state.lock().unwrap().commands_seen.len();
    eprintln!("arming at count={n}");
    fake.state.lock().unwrap().behavior.drop_after_n = Some(n + 1);
    for i in 0..8 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let seen = fake.state.lock().unwrap().commands_seen.clone();
        let idles = seen.iter().filter(|c| c.starts_with("IDLE")).count();
        eprintln!("t={}ms idles={idles} total={} tail={:?}", (i+1)*500, seen.len(), seen.iter().rev().take(3).collect::<Vec<_>>());
    }
    h.abort();
}
