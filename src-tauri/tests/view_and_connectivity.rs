//! P3.6 All Mail membership and P4.6 connectivity/coordinator behaviour.
use sift::app_state::AppState;
use sift::db::Db;
use sift::dto::{ThreadsQuery, View};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

async fn seeded(dir: &std::path::Path) -> (Db, String) {
    let db = Db::open(dir).unwrap();
    let acc = db.new_account("view@x.com", None, None).await.unwrap();
    let aid = acc.id.clone();
    db.write({
        let aid = aid.clone();
        move |c| {
            for (mid, tid, date, subj) in [
                ("inbox-1", "t-inbox", 10, "Inbox message"),
                ("arch-1", "t-arch", 20, "Archived message"),
                ("trash-1", "t-trash", 30, "Trashed message"),
                ("junk-1", "t-junk", 40, "Junk message"),
                ("draft-1", "t-draft", 50, "Draft message"),
                ("mixed-in", "t-mixed", 60, "Mixed inbox"),
                ("mixed-tr", "t-mixed", 61, "Mixed trashed"),
            ] {
                c.execute(
                    "INSERT INTO messages (id,account_id,thread_id,internal_date,subject,snippet,is_draft,label_ids)
                     VALUES (?,?,?,?,?,'snippet',?, '[]')",
                    rusqlite::params![mid, aid, tid, date, subj, mid == "draft-1"],
                )?;
            }
            c.execute(
                "INSERT INTO message_labels (account_id,message_id,label_id) VALUES
                   (?1,'inbox-1','INBOX'),(?1,'inbox-1','UNREAD'),
                   (?1,'trash-1','TRASH'),
                   (?1,'junk-1','SPAM'),
                   (?1,'draft-1','DRAFT'),
                   (?1,'mixed-in','INBOX'),(?1,'mixed-tr','TRASH')",
                rusqlite::params![aid],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();
    for tid in [
        "t-inbox", "t-arch", "t-trash", "t-junk", "t-draft", "t-mixed",
    ] {
        db.recompute_thread(&aid, tid).await.unwrap();
    }
    (db, aid)
}

async fn view_ids(db: &Db, account: &str, view: View) -> Vec<String> {
    let page = db
        .threads_query(ThreadsQuery {
            account_ids: vec![account.to_string()],
            view,
            cursor: None,
            limit: 50,
            unread_only: false,
            has_attachment: false,
        })
        .await
        .unwrap();
    let mut ids: Vec<String> = page.rows.into_iter().map(|r| r.id).collect();
    ids.sort();
    ids
}

#[tokio::test]
async fn p36_all_mail_membership_is_message_level() {
    let dir = tempfile::tempdir().unwrap();
    let (db, aid) = seeded(dir.path()).await;

    let inbox = view_ids(&db, &aid, View::Inbox).await;
    assert!(inbox.contains(&"t-inbox".to_string()));
    assert!(inbox.contains(&"t-mixed".to_string()));

    let archive = view_ids(&db, &aid, View::Archive).await;
    assert!(archive.contains(&"t-arch".to_string()));
    assert!(!archive.contains(&"t-inbox".to_string()));
    assert!(!archive.contains(&"t-draft".to_string()), "drafts are not Archive");

    let all_mail = view_ids(&db, &aid, View::AllMail).await;
    assert!(
        all_mail.contains(&"t-inbox".to_string()),
        "an inbox message is in All Mail: {all_mail:?}"
    );
    assert!(
        all_mail.contains(&"t-arch".to_string()),
        "an archived message is in All Mail"
    );
    assert!(
        all_mail.contains(&"t-mixed".to_string()),
        "a thread is in All Mail when ANY message is outside Trash/Junk"
    );
    assert!(
        !all_mail.contains(&"t-trash".to_string()),
        "Trash is excluded at message level"
    );
    assert!(
        !all_mail.contains(&"t-junk".to_string()),
        "Junk is excluded at message level"
    );
    // Sent-only mail belongs to All Mail too (the label is a message label).
    db.apply_label_change(
        &sift::dto::MessageRef::new(&aid, "arch-1"),
        &["SENT".into()],
        &[],
    )
    .await
    .unwrap();
    let all_mail = view_ids(&db, &aid, View::AllMail).await;
    assert!(all_mail.contains(&"t-arch".to_string()));
    let sent = view_ids(&db, &aid, View::Sent).await;
    assert!(sent.contains(&"t-arch".to_string()));

    // All Mail is a mailbox scope, not an account scope: another account's
    // rows never appear through it.
    let other = db.new_account("other@x.com", None, None).await.unwrap();
    db.write({
        let oid = other.id.clone();
        move |c| {
            c.execute(
                "INSERT INTO messages (id,account_id,thread_id,internal_date,subject,snippet,label_ids)
                 VALUES ('other-1',?,'t-other',70,'Other','s','[]')",
                rusqlite::params![oid],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();
    db.recompute_thread(&other.id, "t-other").await.unwrap();
    let scoped = view_ids(&db, &aid, View::AllMail).await;
    assert!(!scoped.contains(&"t-other".to_string()));
}

fn state(dir: &std::path::Path) -> AppState {
    let db = Db::open(dir).unwrap();
    AppState::new(db, dir.to_path_buf())
}

/// Connectivity is per account: one bad account never marks a healthy one, and
/// recovery clears only the affected account's error (P4.6).
#[tokio::test]
async fn p46_connectivity_is_per_account_and_cached_reads_never_block() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(dir.path());
    let a = st.db.new_account("bad@x.com", None, None).await.unwrap();
    let b = st.db.new_account("good@x.com", None, None).await.unwrap();
    // A cached read works while everything is "offline".
    st.set_network_reachable(false);
    let rows = st
        .db
        .threads_query(ThreadsQuery {
            account_ids: vec![a.id.clone()],
            view: View::Inbox,
            cursor: None,
            limit: 10,
            unread_only: false,
            has_attachment: false,
        })
        .await
        .unwrap();
    assert!(rows.rows.is_empty());
    assert!(st.network_paused(&a.id), "offline pauses background work");
    assert!(st.network_paused(&b.id));

    // The host reports the network is back: the hint alone is not evidence.
    st.set_network_reachable(true);
    assert!(!st.network_paused(&a.id));
    let failing = sift::errors::SiftError::app("imap_transient", "temporary", true);
    st.record_provider_error(&a.id, &failing);
    st.record_provider_ok(&b.id);
    let rows = st.connectivity.snapshot(&[a.id.clone(), b.id.clone()], 0);
    let by_id = |id: &str| {
        rows.iter()
            .find(|r| r.account_id == id)
            .map(|r| r.state.clone())
            .unwrap()
    };
    assert_eq!(by_id(&a.id), sift::connectivity::OFFLINE);
    assert_eq!(by_id(&b.id), sift::connectivity::ONLINE);
    assert!(st.network_paused(&a.id));
    assert!(!st.network_paused(&b.id));

    // Recovery clears only the affected account.
    st.record_provider_ok(&a.id);
    assert_eq!(
        st.connectivity.state_for(&a.id, 0).state,
        sift::connectivity::ONLINE
    );
    assert!(st.connectivity.state_for(&a.id, 0).last_error.is_none());

    // A reauth requirement is sticky and pauses the account until repaired.
    st.require_reauth(&b.id);
    assert_eq!(
        st.connectivity.state_for(&b.id, 0).state,
        sift::connectivity::REAUTH_REQUIRED
    );
    st.record_provider_error(&b.id, &failing);
    assert_eq!(
        st.connectivity.state_for(&b.id, 0).state,
        sift::connectivity::REAUTH_REQUIRED,
        "a transient blip must not downgrade a sticky auth failure"
    );
    st.record_provider_ok(&b.id);
    assert_eq!(
        st.connectivity.state_for(&b.id, 0).state,
        sift::connectivity::ONLINE
    );
}

/// The emitted failed-outbox count is real, not the hardcoded zero (P4.6).
#[tokio::test]
async fn p46_outbox_failed_count_is_real() {
    let dir = tempfile::tempdir().unwrap();
    let st = state(dir.path());
    let a = st.db.new_account("q@x.com", None, None).await.unwrap();
    let ok = st
        .db
        .outbox_enqueue(&a.id, "modify_labels", "{}", None, 0)
        .await
        .unwrap();
    let bad = st
        .db
        .outbox_enqueue(&a.id, "modify_labels", "{}", None, 0)
        .await
        .unwrap();
    let uncertain = st
        .db
        .outbox_enqueue(&a.id, "send", "{}", None, 0)
        .await
        .unwrap();
    st.db
        .outbox_set(bad, "failed", 8, 0, Some("boom".into()))
        .await
        .unwrap();
    st.db
        .outbox_set(uncertain, "uncertain", 1, 0, None)
        .await
        .unwrap();
    assert_eq!(st.db.outbox_failed_count(&a.id).await.unwrap(), 2);
    assert_eq!(st.db.outbox_pending_count(&a.id).await.unwrap(), 1);
    st.db.outbox_set(ok, "done", 1, 0, None).await.unwrap();
    assert_eq!(st.db.outbox_failed_count(&a.id).await.unwrap(), 2);
    assert_eq!(st.db.outbox_pending_count(&a.id).await.unwrap(), 0);
}

/// A manual refresh and a timer tick that overlap run one tick at a time, with
/// at most one coalesced follow-up (P4.3).
#[tokio::test]
async fn p43_overlapping_triggers_run_serially_with_one_coalesced_tick() {
    let dir = tempfile::tempdir().unwrap();
    let st = Arc::new(state(dir.path()));
    let acc = st.db.new_account("c@x.com", None, None).await.unwrap();
    let coordinator = st.coordinator_for(&acc.id).await;
    let concurrent = Arc::new(AtomicUsize::new(0));
    let max_concurrent = Arc::new(AtomicUsize::new(0));
    let runs = Arc::new(AtomicUsize::new(0));
    let job = {
        let (concurrent, max_concurrent, runs) =
            (concurrent.clone(), max_concurrent.clone(), runs.clone());
        move || {
            let (concurrent, max_concurrent, runs) =
                (concurrent.clone(), max_concurrent.clone(), runs.clone());
            async move {
                let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                max_concurrent.fetch_max(now, Ordering::SeqCst);
                runs.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(80)).await;
                concurrent.fetch_sub(1, Ordering::SeqCst);
            }
        }
    };
    let first = {
        let (coordinator, job) = (coordinator.clone(), job.clone());
        tokio::spawn(async move { coordinator.tick_now(job).await })
    };
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = {
        let (coordinator, job) = (coordinator.clone(), job.clone());
        tokio::spawn(async move { coordinator.tick_now(job).await })
    };
    let first_runs = first.await.unwrap();
    let second_runs = second.await.unwrap();
    assert_eq!(
        (first_runs.max(second_runs), first_runs.min(second_runs)),
        (2, 0),
        "one caller owns the tick (1 pass + 1 coalesced); the other is merged, not queued"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 2, "two passes in total");
    assert_eq!(
        max_concurrent.load(Ordering::SeqCst),
        1,
        "two ticks never run at the same time"
    );
    assert_eq!(coordinator.runs(), 2);

    // A cancelled account's coordinator refuses to start a new pass.
    st.begin_generation(&acc.id).await;
    assert!(coordinator.is_cancelled());
    let n = coordinator.tick_now(job.clone()).await;
    assert_eq!(n, 0);
}
