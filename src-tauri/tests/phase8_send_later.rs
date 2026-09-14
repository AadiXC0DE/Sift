//! Phase 8 P8.1 — Send Later.
//!
//! The contract these tests hold the implementation to: a scheduled message is
//! one immutable revision that leaves **at most once**, when its stored instant
//! arrives and the app is online; the stored instant survives a DST jump, a
//! zone relabelling and a clock that moved backwards; and the schedule can be
//! moved or cancelled right up to the claim, but not after it.
#![allow(clippy::await_holding_lock)] // the shared env server is serialized on purpose

use sift::db::drafts::{SendCancel, SendSchedule};
use sift::db::Db;
use sift::dto::{Address, Draft, DRAFT_STATE_SENT};
use sift::provider::gmail::api::GmailApiProvider;
use sift::provider::gmail::client::GmailClient;
use sift::send_later::{plan, resolve_in, tomorrow_at_in, validate_instant, ScheduleRequest};

static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn addr(email: &str) -> Address {
    Address {
        n: None,
        e: email.into(),
        me: None,
    }
}

fn identity() -> sift::outgoing::Identity {
    sift::outgoing::Identity {
        email: "ada@x.com".into(),
        display_name: Some("Ada".into()),
    }
}

/// The wall-time string the UI sends for an instant, in the platform's zone.
fn local_string(instant: i64) -> String {
    chrono::Local
        .timestamp_millis_opt(instant)
        .single()
        .unwrap()
        .format("%Y-%m-%dT%H:%M")
        .to_string()
}

/// A schedule the user picked for `instant`, exactly as the composer derives it.
fn custom(instant: i64, timezone: &str) -> ScheduleRequest {
    ScheduleRequest {
        kind: "custom".into(),
        not_before: Some(instant),
        local_time: Some(local_string(instant)),
        timezone: Some(timezone.into()),
    }
}

/// One draft queued for `schedule`, as the composer would leave it.
async fn queued(
    db: &Db,
    account_id: &str,
    dir: &std::path::Path,
    schedule: &SendSchedule,
) -> (i64, String) {
    let draft = db
        .drafts_upsert(
            &Draft {
                account_id: account_id.to_string(),
                thread_id: None,
                to_json: vec![addr("bob@y.org")],
                subject: "Scheduled".into(),
                body_html: "<p>later</p>".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let prepared = sift::outgoing::prepare(dir, &draft, &identity(), 1_700_000_000).unwrap();
    let handle = db
        .drafts_enqueue_send(&prepared, schedule, false)
        .await
        .unwrap();
    (handle.op_id, draft.local_id)
}

async fn state_of(db: &Db, op_id: i64) -> String {
    db.outbox_get(op_id).await.unwrap().unwrap().state
}

async fn send_op_count(db: &Db, account_id: &str) -> i64 {
    let a = account_id.to_string();
    db.read(move |c| {
        Ok(c.query_row(
            "SELECT count(*) FROM outbox_ops WHERE account_id=? AND kind='send'",
            rusqlite::params![a],
            |r| r.get(0),
        )?)
    })
    .await
    .unwrap()
}

/// Count the sends the provider actually received, and answer each one.
async fn counting_server() -> (
    wiremock::MockServer,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let server = wiremock::MockServer::start().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/messages/send"))
        .respond_with(move |_req: &wiremock::Request| {
            counter.fetch_add(1, Ordering::SeqCst);
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": "mSent", "threadId": "t1"}))
        })
        .mount(&server)
        .await;
    (server, hits)
}

/// P8.1: the message does not leave before its deadline, leaves once when its
/// deadline arrives, and a further drain finds nothing left to send.
#[tokio::test]
async fn p8_1_a_scheduled_send_waits_then_leaves_exactly_once() {
    let _g = lock_env();
    let (server, hits) = counting_server().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );

    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));

    let now = sift::db::now_ms();
    // A minute out, truncated to the minute so the wall time round-trips.
    let instant = (now + 120_000) / 60_000 * 60_000;
    let schedule = plan(&custom(instant, "UTC"), now).unwrap();
    assert_eq!(schedule.not_before, instant);
    assert_eq!(schedule.scheduled_at, Some(instant));
    assert!(schedule.is_scheduled());
    let (op_id, local_id) = queued(&db, &acc.id, dir.path(), &schedule).await;

    // Sleeping across the due time: the deadline has not arrived.
    assert!(
        !sift::outbox::drain_one(&db, &provider, &acc.id, true)
            .await
            .unwrap(),
        "an operation before its deadline is not claimed"
    );
    assert_eq!(state_of(&db, op_id).await, "pending");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);

    // The clock reaches the deadline.
    db.outbox_set(op_id, "pending", 0, 0, None).await.unwrap();
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    assert_eq!(state_of(&db, op_id).await, "done");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Nothing is left, so a repeated drain cannot resend the revision.
    assert!(!sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one immutable revision leaves once"
    );

    // The sent draft is terminal and no longer claims a deadline of its own.
    let draft = db.drafts_get(&local_id).await.unwrap().unwrap();
    assert_eq!(draft.state, DRAFT_STATE_SENT);
    assert!(draft.not_before.is_none(), "{:?}", draft.not_before);
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// P8.1: "Sift must be running and online. Otherwise this sends when Sift next
/// connects." A deadline that arrives offline leaves the operation queued.
#[tokio::test]
async fn p8_1_an_offline_deadline_stays_queued_until_the_network_returns() {
    let _g = lock_env();
    let (server, hits) = counting_server().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );

    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));

    // Already overdue: the only thing keeping it in is the network.
    let (op_id, _) = queued(
        &db,
        &acc.id,
        dir.path(),
        &SendSchedule::now(sift::db::now_ms() - 60_000),
    )
    .await;
    assert!(
        !sift::outbox::drain_one(&db, &provider, &acc.id, false)
            .await
            .unwrap(),
        "offline: nothing is claimed"
    );
    assert_eq!(state_of(&db, op_id).await, "pending");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);

    assert!(
        sift::outbox::drain_one(&db, &provider, &acc.id, true)
            .await
            .unwrap(),
        "the overdue message leaves when the network returns"
    );
    assert_eq!(state_of(&db, op_id).await, "done");
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    std::env::remove_var("SIFT_GMAIL_BASE");
}

// -- DST --------------------------------------------------------------------
//
// A zone with a known transition, modelled explicitly: the skipped and repeated
// hours then resolve the same way on every machine, whatever zone the machine
// itself runs in.

use chrono::{Duration as ChronoDuration, FixedOffset, LocalResult, NaiveDateTime, TimeZone};

/// A zone running at `before` until its wall clock reaches `local_transition`,
/// then at `after` — a gap when the clock jumps forward, a repeat when it jumps
/// back.
#[derive(Clone, Copy, Debug)]
struct SyntheticZone {
    transition_utc_ms: i64,
    local_transition: NaiveDateTime,
    before: i32,
    after: i32,
    gap: bool,
}

fn offset(seconds: i32) -> FixedOffset {
    FixedOffset::east_opt(seconds).unwrap()
}

fn utc(rfc3339: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .timestamp_millis()
}

fn naive(rfc3339: &str) -> NaiveDateTime {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .naive_utc()
}

impl SyntheticZone {
    /// 2026-03-08T02:00 local does not exist: 02:00 becomes 03:00 EDT.
    fn spring_forward() -> Self {
        Self {
            transition_utc_ms: utc("2026-03-08T07:00:00Z"),
            local_transition: naive("2026-03-08T02:00:00Z"),
            before: -5 * 3600,
            after: -4 * 3600,
            gap: true,
        }
    }

    /// 2026-11-01T01:00 local happens twice: 02:00 becomes 01:00 EST.
    fn fall_back() -> Self {
        Self {
            transition_utc_ms: utc("2026-11-01T06:00:00Z"),
            local_transition: naive("2026-11-01T02:00:00Z"),
            before: -4 * 3600,
            after: -5 * 3600,
            gap: false,
        }
    }

    fn offset_of(&self, utc_ms: i64) -> FixedOffset {
        offset(if utc_ms < self.transition_utc_ms {
            self.before
        } else {
            self.after
        })
    }
}

impl TimeZone for SyntheticZone {
    type Offset = FixedOffset;

    /// A zone with no transition, for the one offset it was built from.
    fn from_offset(off: &FixedOffset) -> Self {
        let seconds = off.local_minus_utc();
        Self {
            transition_utc_ms: 0,
            local_transition: NaiveDateTime::MIN,
            before: seconds,
            after: seconds,
            gap: false,
        }
    }

    fn offset_from_local_date(&self, local: &chrono::NaiveDate) -> LocalResult<FixedOffset> {
        // Noon is never inside one of these transitions, so the date's offset
        // is the offset of its midday.
        LocalResult::Single(
            self.offset_of(
                local
                    .and_hms_opt(12, 0, 0)
                    .unwrap()
                    .and_utc()
                    .timestamp_millis(),
            ),
        )
    }

    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<FixedOffset> {
        if self.gap {
            let end =
                self.local_transition + ChronoDuration::seconds((self.after - self.before) as i64);
            if *local >= self.local_transition && *local < end {
                return LocalResult::None;
            }
        } else {
            let start =
                self.local_transition - ChronoDuration::seconds((self.before - self.after) as i64);
            if *local >= start && *local < self.local_transition {
                // The earlier occurrence comes first in UTC.
                return LocalResult::Ambiguous(offset(self.before), offset(self.after));
            }
        }
        LocalResult::Single(offset(if *local < self.local_transition {
            self.before
        } else {
            self.after
        }))
    }

    fn offset_from_utc_date(&self, utc: &chrono::NaiveDate) -> FixedOffset {
        self.offset_of(
            utc.and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis(),
        )
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
        self.offset_of(utc.and_utc().timestamp_millis())
    }
}

/// P8.1: a local time that does not exist (spring forward) resolves to the
/// first instant that does — the transition — never to nothing.
#[test]
fn p8_1_a_skipped_local_time_resolves_after_the_gap() {
    let zone = SyntheticZone::spring_forward();
    let skipped = naive("2026-03-08T02:30:00Z");
    let resolved =
        resolve_in(&zone, skipped).expect("a skipped wall time still resolves to a real instant");
    // 02:30 does not exist; the first instant after the gap is 03:00 EDT.
    assert_eq!(resolved.timestamp_millis(), utc("2026-03-08T07:00:00Z"));

    // The same case through the entry point the composer uses: "tomorrow 02:00"
    // asked for on the day before the transition.
    let now = utc("2026-03-08T01:00:00Z"); // 2026-03-07T20:00 EST
    let (instant, local) = tomorrow_at_in(&zone, now, 2).unwrap();
    assert_eq!(local, "2026-03-08T03:00");
    assert_eq!(instant, utc("2026-03-08T07:00:00Z"));
}

/// P8.1: a local time that happens twice (fall back) resolves to its first
/// occurrence, so the send cannot fire on the second pass.
#[test]
fn p8_1_a_repeated_local_time_resolves_to_the_first_occurrence() {
    let zone = SyntheticZone::fall_back();
    let repeated = naive("2026-11-01T01:30:00Z");
    let resolved = resolve_in(&zone, repeated).unwrap();
    // 01:30 EDT (-4) is the first occurrence: 05:30Z, not 06:30Z.
    assert_eq!(resolved.timestamp_millis(), utc("2026-11-01T05:30:00Z"));

    let now = utc("2026-11-01T00:00:00Z"); // 2026-10-31T20:00 EDT
    let (instant, local) = tomorrow_at_in(&zone, now, 1).unwrap();
    assert_eq!(local, "2026-11-01T01:00");
    assert_eq!(instant, utc("2026-11-01T05:00:00Z"));
}

/// P8.1: the zone is a label on a stored instant, not an input to it. Changing
/// the zone Sift reports cannot move a scheduled send.
#[tokio::test]
async fn p8_1_a_timezone_change_does_not_move_the_stored_instant() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();

    let now = sift::db::now_ms();
    let instant = (now + 3_600_000) / 60_000 * 60_000;
    let berlin = plan(&custom(instant, "Europe/Berlin"), now).unwrap();
    let (op_id, _) = queued(&db, &acc.id, dir.path(), &berlin).await;

    // The user lands in another zone: the same instant, relabelled.
    let moved = plan(
        &ScheduleRequest {
            kind: "custom".into(),
            not_before: Some(instant),
            local_time: Some(local_string(instant)),
            timezone: Some("America/New_York".into()),
        },
        now,
    )
    .unwrap();
    db.drafts_reschedule_send(op_id, moved.clone())
        .await
        .unwrap();

    let stored = db.outbox_get(op_id).await.unwrap().unwrap();
    assert_eq!(stored.not_before, instant, "the deadline is absolute");
    assert_eq!(moved.not_before, berlin.not_before);
    assert_eq!(moved.scheduled_at, Some(instant));
    assert_eq!(
        moved.scheduled_timezone.as_deref(),
        Some("America/New_York")
    );
    // The description names the stored zone, so the displayed time is never
    // ambiguous about which wall clock it belongs to.
    let described = sift::send_later::describe(
        moved.scheduled_local_time.as_deref(),
        moved.scheduled_timezone.as_deref(),
    );
    assert!(described.contains("America/New_York"), "{described}");
}

/// P8.1: a clock that moved backwards does not move the deadline, and does not
/// let the message out early.
#[tokio::test]
async fn p8_1_a_clock_moved_backward_keeps_the_deadline() {
    let now = sift::db::now_ms();
    let instant = now + 3_600_000;
    let schedule = plan(&custom(instant, "UTC"), now).unwrap();
    // The device clock is now an hour behind the instant it stored.
    let behind = now - 3_600_000;
    assert!(validate_instant(instant, behind).is_ok());
    let again = plan(&custom(instant, "UTC"), behind).unwrap();
    assert_eq!(again.not_before, schedule.not_before);

    // And an operation queued for that instant is not claimable while the
    // clock is behind it.
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let (op_id, _) = queued(&db, &acc.id, dir.path(), &schedule).await;
    assert!(
        db.outbox_claim(&acc.id).await.unwrap().is_none(),
        "a future deadline is not claimed"
    );
    assert_eq!(state_of(&db, op_id).await, "pending");
}

/// P8.1: editing the time at the claim boundary moves the one operation rather
/// than queueing a second; once it is claimed, neither an edit nor a cancel can
/// reach it.
#[tokio::test]
async fn p8_1_edits_and_cancels_stop_at_the_claim_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();

    let now = sift::db::now_ms();
    let instant = (now + 3_600_000) / 60_000 * 60_000;
    let (op_id, _) = queued(
        &db,
        &acc.id,
        dir.path(),
        &plan(&custom(instant, "UTC"), now).unwrap(),
    )
    .await;

    // Edit before the claim: the same operation moves, and exactly one send
    // exists for the revision.
    let later = instant + 3_600_000;
    let handle = db
        .drafts_reschedule_send(op_id, plan(&custom(later, "UTC"), now).unwrap())
        .await
        .unwrap();
    assert_eq!(
        handle.op_id, op_id,
        "editing moves the operation, it does not add one"
    );
    assert_eq!(handle.not_before, later);
    assert_eq!(send_op_count(&db, &acc.id).await, 1);

    // Cancel before the claim returns the reopened draft.
    match db.drafts_cancel_send_detailed(op_id).await.unwrap() {
        SendCancel::Cancelled(draft) => assert_eq!(draft.state, "editing"),
        other => panic!("a pending send must be cancellable: {other:?}"),
    }

    // A claimed operation is out of reach for both, with the state named.
    let (claimed_op, _) = queued(
        &db,
        &acc.id,
        dir.path(),
        &plan(&custom(later, "UTC"), now).unwrap(),
    )
    .await;
    db.outbox_set(claimed_op, "pending", 0, 0, None)
        .await
        .unwrap();
    let claimed = db.outbox_claim(&acc.id).await.unwrap().expect("due now");
    assert_eq!(claimed.id, claimed_op);
    let err = db
        .drafts_reschedule_send(
            claimed_op,
            plan(&custom(later + 60_000, "UTC"), now).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.downcast_ref::<sift::errors::SiftError>()
            .unwrap()
            .code(),
        "send_already_claimed"
    );
    match db.drafts_cancel_send_detailed(claimed_op).await.unwrap() {
        SendCancel::TooLate { state } => assert_eq!(state, "inflight"),
        other => panic!("a claimed send must not be cancellable: {other:?}"),
    }
}
