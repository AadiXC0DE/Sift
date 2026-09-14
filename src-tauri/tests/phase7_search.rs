//! Phase 7 acceptance — search semantics, paging and saved searches.
//!
//! Every fixture is deterministic and seeded through the real upsert path, so
//! the tests exercise the same SQL the app runs.

use sift::db::messages::MsgUpsert;
use sift::db::Db;
use sift::dto::{Label, SavedSearchInput, ThreadsQuery, View};
use sift::search::{local, query};

#[derive(Default)]
struct Spec<'a> {
    id: &'a str,
    thread: &'a str,
    at: i64,
    subject: &'a str,
    from: (&'a str, &'a str),
    to: &'a [&'a str],
    cc: &'a [&'a str],
    labels: &'a [&'a str],
    unread: bool,
    starred: bool,
    has_attachments: bool,
    /// Indexed body text, so free-text search has something to match.
    body: &'a str,
}

fn addrs(list: &[&str]) -> String {
    let rows: Vec<serde_json::Value> = list
        .iter()
        .map(|e| serde_json::json!({"n": null, "e": e, "me": null}))
        .collect();
    serde_json::to_string(&rows).unwrap()
}

async fn seed(db: &Db, account_id: &str, spec: &Spec<'_>) {
    db.messages_upsert(MsgUpsert {
        id: spec.id.to_string(),
        account_id: account_id.to_string(),
        thread_id: spec.thread.to_string(),
        internal_date: spec.at,
        from_name: Some(spec.from.0.to_string()),
        from_email: Some(spec.from.1.to_string()),
        to_json: addrs(spec.to),
        cc_json: addrs(spec.cc),
        subject: spec.subject.to_string(),
        snippet: spec.body.chars().take(80).collect(),
        has_attachments: spec.has_attachments,
        is_unread: spec.unread,
        is_starred: spec.starred,
        label_ids: spec.labels.iter().map(|l| l.to_string()).collect(),
        ..Default::default()
    })
    .await
    .unwrap();
    let (account, id, body) = (
        account_id.to_string(),
        spec.id.to_string(),
        spec.body.to_string(),
    );
    db.write(move |c| {
        c.execute(
            "UPDATE messages_fts SET body=? WHERE account_id=? AND message_id=?",
            rusqlite::params![body, account, id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn label(db: &Db, account_id: &str, id: &str, name: &str) {
    db.labels_upsert(&Label {
        account_id: account_id.to_string(),
        id: id.to_string(),
        name: name.to_string(),
        kind: "user".into(),
        color_bg: None,
        color_fg: None,
        visible: true,
        unread_count: 0,
        total_count: 0,
        sort_order: 1,
        ..Default::default()
    })
    .await
    .unwrap();
}

/// Run one local search and return the `(account, thread)` pairs it matched.
async fn matches(db: &Db, accounts: &[String], q: &str) -> Vec<(String, String)> {
    let parsed = query::parse(q);
    let page = local::search_page(db, accounts, &parsed, None, "test", 100)
        .await
        .unwrap();
    page.rows
        .into_iter()
        .map(|r| (r.account_id, r.id))
        .collect()
}

fn sorted(mut rows: Vec<(String, String)>) -> Vec<(String, String)> {
    rows.sort();
    rows
}

fn ids(pairs: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = pairs.iter().map(|(_, thread)| thread.clone()).collect();
    out.sort();
    out
}

/// Build the shared fixture mailbox used by the operator table.
async fn fixture(dir: &std::path::Path) -> (Db, String, String) {
    let db = Db::open(dir).unwrap();
    let primary = db.new_account("ada@example.com", None, None).await.unwrap();
    let second = db.new_account("bob@example.com", None, None).await.unwrap();
    label(&db, &primary.id, "Label_12345", "Client Work").await;
    label(&db, &primary.id, "Label_Hier", "Projects/2026").await;
    label(&db, &primary.id, "Label_VIP", "VIP").await;
    let a = primary.id.clone();
    // t1: unread, tagged, carries an attachment, from Ada to Bob.
    seed(
        &db,
        &a,
        &Spec {
            id: "m1",
            thread: "t1",
            at: 1_000,
            subject: "Q3 numbers",
            from: ("Ada Lovelace", "ada@example.com"),
            to: &["bob@example.com"],
            labels: &["INBOX", "UNREAD", "Label_VIP"],
            unread: true,
            has_attachments: true,
            body: "the quarterly numbers are attached",
            ..Default::default()
        },
    )
    .await;
    // t2: read, Client Work, cc Carol, "café 日本語" punctuation sample.
    seed(
        &db,
        &a,
        &Spec {
            id: "m2",
            thread: "t2",
            at: 2_000,
            subject: "Re: Q3 numbers",
            from: ("Carol Danvers", "carol@example.com"),
            to: &["ada@example.com"],
            cc: &["carol@example.com"],
            labels: &["INBOX", "Label_12345"],
            body: "quarterly numbers filed; café 日本語 C++",
            ..Default::default()
        },
    )
    .await;
    // t3: sent by Ada under the same label.
    seed(
        &db,
        &a,
        &Spec {
            id: "m3",
            thread: "t3",
            at: 3_000,
            subject: "unrelated",
            from: ("Ada Lovelace", "ada@example.com"),
            to: &["dave@example.com"],
            labels: &["SENT", "Label_12345"],
            body: "hello quarterly",
            ..Default::default()
        },
    )
    .await;
    // t4: junk.
    seed(
        &db,
        &a,
        &Spec {
            id: "m4",
            thread: "t4",
            at: 4_000,
            subject: "junk",
            from: ("Spam", "spam@junk.example"),
            to: &["ada@example.com"],
            labels: &["SPAM"],
            body: "quarterly numbers",
            ..Default::default()
        },
    )
    .await;
    // t5: trashed, still from Ada.
    seed(
        &db,
        &a,
        &Spec {
            id: "m5",
            thread: "t5",
            at: 5_000,
            subject: "Trashed",
            from: ("Ada Lovelace", "ada@example.com"),
            to: &["ada@example.com"],
            labels: &["TRASH"],
            body: "quarterly numbers",
            ..Default::default()
        },
    )
    .await;
    // t6: a mixed-label thread - one tagged inbox message, one trashed.
    seed(
        &db,
        &a,
        &Spec {
            id: "m6",
            thread: "t6",
            at: 6_000,
            subject: "Mixed",
            from: ("Ada Lovelace", "ada@example.com"),
            to: &["ada@example.com"],
            labels: &["INBOX", "Label_12345"],
            body: "quarterly numbers",
            ..Default::default()
        },
    )
    .await;
    seed(
        &db,
        &a,
        &Spec {
            id: "m7",
            thread: "t6",
            at: 6_100,
            subject: "Mixed",
            from: ("Ada Lovelace", "ada@example.com"),
            to: &["ada@example.com"],
            labels: &["TRASH"],
            body: "quarterly numbers",
            ..Default::default()
        },
    )
    .await;
    // t7: the second account holds the same provider thread id.
    seed(
        &db,
        &second.id,
        &Spec {
            id: "m8",
            thread: "t1",
            at: 7_000,
            subject: "Second account",
            from: ("Ada Lovelace", "ada@example.com"),
            to: &["bob@example.com"],
            labels: &["INBOX"],
            starred: true,
            body: "quarterly numbers here too",
            ..Default::default()
        },
    )
    .await;
    (db, a, second.id.clone())
}

#[tokio::test]
async fn p7_t20_operator_table() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a, b) = fixture(dir.path()).await;
    let both = vec![a.clone(), b.clone()];
    let one = vec![a.clone()];

    struct Row<'a> {
        q: &'a str,
        accounts: &'a Vec<String>,
        expect: &'a [&'a str],
    }
    let rows = [
        Row {
            q: "from:ada",
            accounts: &one,
            expect: &["t1", "t3", "t6"],
        },
        Row {
            q: "from:Lovelace",
            accounts: &one,
            expect: &["t1", "t3", "t6"],
        },
        Row {
            q: "from:carol@example.com",
            accounts: &one,
            expect: &["t2"],
        },
        Row {
            q: "to:bob@example.com",
            accounts: &one,
            expect: &["t1"],
        },
        Row {
            q: "cc:carol",
            accounts: &one,
            expect: &["t2"],
        },
        Row {
            q: "subject:Q3 numbers",
            accounts: &one,
            expect: &["t1", "t2"],
        },
        Row {
            q: "subject:\"Q3 numbers\"",
            accounts: &one,
            expect: &["t1", "t2"],
        },
        // Label by user-visible name, hierarchical name, and provider id.
        Row {
            q: "label:\"Client Work\"",
            accounts: &one,
            expect: &["t2", "t3", "t6"],
        },
        Row {
            q: "label:Label_12345",
            accounts: &one,
            expect: &["t2", "t3", "t6"],
        },
        Row {
            q: "label:label_12345",
            accounts: &one,
            expect: &["t2", "t3", "t6"],
        },
        // A single-word label matches by name and by provider id, ignoring case.
        Row {
            q: "label:vip",
            accounts: &one,
            expect: &["t1"],
        },
        Row {
            q: "label:VIP",
            accounts: &one,
            expect: &["t1"],
        },
        Row {
            q: "label:Label_VIP",
            accounts: &one,
            expect: &["t1"],
        },
        Row {
            q: "label:label_vip",
            accounts: &one,
            expect: &["t1"],
        },
        Row {
            q: "label:\"Projects/2026\"",
            accounts: &one,
            expect: &[],
        },
        // Mailbox scopes; Trash and Junk are excluded unless named.
        Row {
            q: "in:inbox",
            accounts: &one,
            expect: &["t1", "t2", "t6"],
        },
        Row {
            q: "in:sent",
            accounts: &one,
            expect: &["t3"],
        },
        Row {
            q: "in:trash",
            accounts: &one,
            expect: &["t5", "t6"],
        },
        Row {
            q: "in:spam",
            accounts: &one,
            expect: &["t4"],
        },
        Row {
            q: "in:archive",
            accounts: &one,
            expect: &["t3"],
        },
        Row {
            q: "quarterly",
            accounts: &one,
            expect: &["t1", "t2", "t3", "t6"],
        },
        Row {
            q: "quarterly in:spam",
            accounts: &one,
            expect: &["t4"],
        },
        Row {
            q: "quarterly in:trash",
            accounts: &one,
            expect: &["t5", "t6"],
        },
        Row {
            q: "has:attachment",
            accounts: &both,
            expect: &["t1"],
        },
        Row {
            q: "is:read",
            accounts: &one,
            expect: &["t2", "t3", "t6"],
        },
        Row {
            q: "is:unread",
            accounts: &one,
            expect: &["t1"],
        },
        Row {
            q: "is:starred",
            accounts: &both,
            expect: &["t1"],
        },
        Row {
            q: "-from:ada",
            accounts: &one,
            expect: &["t2"],
        },
        Row {
            q: "quarterly -from:ada",
            accounts: &one,
            expect: &["t2"],
        },
        Row {
            q: "from:ada OR from:carol",
            accounts: &one,
            expect: &["t1", "t2", "t3", "t6"],
        },
        Row {
            q: "quarterly -in:inbox",
            accounts: &one,
            expect: &["t3"],
        },
        // Account scope is part of the query.
        Row {
            q: "from:ada",
            accounts: &both,
            expect: &["t1", "t1", "t3", "t6"],
        },
    ];
    for row in rows {
        let got = ids(&matches(&db, row.accounts, row.q).await);
        let mut want: Vec<String> = row.expect.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "query {:?}", row.q);
    }

    // The second account's own thread id is a distinct row.
    let scoped = sorted(matches(&db, &both, "from:ada").await);
    assert!(scoped.contains(&(b, "t1".to_string())));
    assert!(scoped.contains(&(a.clone(), "t1".to_string())));
}

#[tokio::test]
async fn p7_t21_phrases_versus_separated_words_punctuation_and_scripts() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("a@example.com", None, None).await.unwrap();
    let a = account.id.clone();
    let cases = [
        ("p1", "u1", "alpha beta gamma delta"),
        ("p2", "u2", "alpha something beta"),
        ("p3", "u3", "café crème"),
        ("p4", "u4", "日本語のメール"),
        ("p5", "u5", "C++ and C# and a.b@c.com"),
        ("p6", "u6", "100% pure"),
    ];
    for (id, thread, body) in cases {
        seed(
            &db,
            &a,
            &Spec {
                id,
                thread,
                at: 1,
                subject: body,
                from: ("A", "a@example.com"),
                body,
                ..Default::default()
            },
        )
        .await;
    }
    // A quoted phrase requires the words in order and adjacent.
    assert_eq!(
        ids(&matches(&db, std::slice::from_ref(&a), "\"alpha beta\"").await),
        vec!["u1"]
    );
    // Separated words match anywhere in the message.
    assert_eq!(
        ids(&matches(&db, std::slice::from_ref(&a), "alpha beta").await),
        vec!["u1", "u2"]
    );
    // The quoted value of a subject keeps its spaces.
    assert_eq!(
        ids(&matches(
            &db,
            std::slice::from_ref(&a),
            "subject:\"alpha beta gamma\""
        )
        .await),
        vec!["u1"]
    );
    // Accents, CJK and punctuation are searchable and do not break the query.
    assert_eq!(
        ids(&matches(&db, std::slice::from_ref(&a), "café").await),
        vec!["u3"]
    );
    assert_eq!(
        ids(&matches(&db, std::slice::from_ref(&a), "日本語").await),
        vec!["u4"]
    );
    assert_eq!(
        ids(&matches(&db, std::slice::from_ref(&a), "C++").await),
        vec!["u5"]
    );
    assert_eq!(
        ids(&matches(&db, std::slice::from_ref(&a), "100%").await),
        vec!["u6"]
    );
    for hostile in [
        "\"", "*", "NEAR", "AND", "OR", "()", "a:b", "-", "^^", "\"\"\"",
    ] {
        // Never an error, and never a silently widened result set.
        let got = matches(&db, std::slice::from_ref(&a), hostile).await;
        assert!(got.len() <= cases.len(), "{hostile}");
    }
}

#[tokio::test]
async fn p7_t22_dates_are_local_calendar_days() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("a@example.com", None, None).await.unwrap();
    let a = account.id.clone();
    let mar1 = query::local_day_start_ms(2026, 3, 1).unwrap();
    let mar2 = query::local_day_start_ms(2026, 3, 2).unwrap();
    for (id, thread, at) in [
        ("d0", "before", mar1 - 1),
        ("d1", "on", mar1),
        ("d2", "inside", mar1 + 60_000),
        ("d3", "after", mar2),
    ] {
        seed(
            &db,
            &a,
            &Spec {
                id,
                thread,
                at,
                subject: "dated",
                from: ("A", "a@example.com"),
                labels: &["INBOX"],
                body: "dated",
                ..Default::default()
            },
        )
        .await;
    }
    // after includes the start of that local day.
    assert_eq!(
        ids(&matches(&db, std::slice::from_ref(&a), "after:2026-03-01").await),
        vec!["after", "inside", "on"]
    );
    // before excludes the start of that local day.
    assert_eq!(
        ids(&matches(&db, std::slice::from_ref(&a), "before:2026-03-01").await),
        vec!["before"]
    );
    assert_eq!(
        ids(&matches(
            &db,
            std::slice::from_ref(&a),
            "after:2026-03-01 before:2026-03-02"
        )
        .await),
        vec!["inside", "on"]
    );
}

#[tokio::test]
async fn p7_t23_invalid_date_is_a_hint_and_never_broadens() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a, _) = fixture(dir.path()).await;
    let parsed = query::parse("before:tomorrow");
    assert!(parsed.hints.iter().any(|h| h.kind == "invalid_date"));
    assert!(parsed.is_fatal());
    let page = local::search_page(&db, std::slice::from_ref(&a), &parsed, None, "test", 100)
        .await
        .unwrap();
    assert!(
        page.rows.is_empty(),
        "an unparsable date must not return the whole mailbox"
    );
    // A valid date in the same query still compiles; only the broken predicate
    // is unmatchable.
    let mixed = query::parse("from:ada before:tomorrow");
    let page = local::search_page(&db, &[a], &mixed, None, "test", 100)
        .await
        .unwrap();
    assert!(page.rows.is_empty());
}

#[tokio::test]
async fn p7_t24_a_long_conversation_cannot_hide_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("a@example.com", None, None).await.unwrap();
    let a = account.id.clone();
    // 400 matching messages in one conversation...
    for i in 0..400 {
        seed(
            &db,
            &a,
            &Spec {
                id: &format!("bulk-{i}"),
                thread: "long",
                at: 1_000 + i as i64,
                subject: "bulk",
                from: ("Bulk", "bulk@example.com"),
                labels: &["INBOX"],
                body: "needle in the long conversation",
                ..Default::default()
            },
        )
        .await;
    }
    // ...and one newer qualifying conversation that must still be visible.
    seed(
        &db,
        &a,
        &Spec {
            id: "solo",
            thread: "solo",
            at: 9_000,
            subject: "solo",
            from: ("Solo", "solo@example.com"),
            labels: &["INBOX"],
            body: "needle in a short conversation",
            ..Default::default()
        },
    )
    .await;
    let page = local::search_page(
        &db,
        std::slice::from_ref(&a),
        &query::parse("needle"),
        None,
        "s",
        100,
    )
    .await
    .unwrap();
    let threads = ids(&page
        .rows
        .iter()
        .map(|r| (r.account_id.clone(), r.id.clone()))
        .collect::<Vec<_>>());
    assert_eq!(threads, vec!["long", "solo"]);
    assert!(page.next_cursor.is_none());
}

#[tokio::test]
async fn p7_t25_search_beyond_one_hundred_pages_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("a@example.com", None, None).await.unwrap();
    let a = account.id.clone();
    for i in 0..150 {
        seed(
            &db,
            &a,
            &Spec {
                id: &format!("s-{i}"),
                thread: &format!("thread-{i}"),
                at: 1_000 + i as i64,
                subject: "pageable",
                from: ("A", "a@example.com"),
                labels: &["INBOX"],
                body: "pageable body",
                ..Default::default()
            },
        )
        .await;
    }
    let parsed = query::parse("pageable");
    let mut seen: Vec<String> = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = local::search_page(
            &db,
            std::slice::from_ref(&a),
            &parsed,
            cursor.as_ref(),
            "scope",
            100,
        )
        .await
        .unwrap();
        pages += 1;
        seen.extend(page.rows.iter().map(|r| r.id.clone()));
        match page.next_cursor {
            Some(next) => {
                cursor = Some(
                    sift::search::cursor::expect(
                        &next,
                        sift::search::cursor::SortKind::Search,
                        "scope",
                        &parsed.fingerprint(),
                    )
                    .unwrap(),
                );
            }
            None => break,
        }
        assert!(pages < 10, "paging must terminate");
    }
    assert_eq!(pages, 2);
    let unique: std::collections::HashSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), 150);
    assert_eq!(seen.len(), 150, "no thread may be returned twice");
}

#[tokio::test]
async fn p7_t26_same_timestamp_threads_across_accounts_page_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let first = db.new_account("one@example.com", None, None).await.unwrap();
    let second = db.new_account("two@example.com", None, None).await.unwrap();
    let accounts = vec![first.id.clone(), second.id.clone()];
    // 251 threads that all share one timestamp: only a complete sort tuple
    // makes paging deterministic.
    for i in 0..251 {
        let owner = if i % 2 == 0 { &first.id } else { &second.id };
        seed(
            &db,
            owner,
            &Spec {
                id: &format!("m-{i}"),
                thread: &format!("t-{i:03}"),
                at: 5_000,
                subject: "same instant",
                from: ("A", "a@example.com"),
                labels: &["INBOX"],
                body: "same instant",
                ..Default::default()
            },
        )
        .await;
    }
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut cursor: Option<sift::search::cursor::Cursor> = None;
    let mut pages = 0;
    loop {
        let page = db
            .threads_query(ThreadsQuery {
                account_ids: accounts.clone(),
                view: View::Inbox,
                cursor: cursor.as_ref().map(sift::search::cursor::encode),
                limit: 100,
                unread_only: false,
                has_attachment: false,
            })
            .await
            .unwrap();
        pages += 1;
        seen.extend(
            page.rows
                .iter()
                .map(|r| (r.account_id.clone(), r.id.clone())),
        );
        match page.next_cursor {
            Some(raw) => {
                let (sort, scope, q) = sift::db::threads::page_identity(&View::Inbox, &accounts);
                cursor = Some(sift::search::cursor::expect(&raw, sort, &scope, &q).unwrap());
            }
            None => break,
        }
        assert!(pages < 10, "paging must terminate");
    }
    assert_eq!(pages, 3, "251 rows at 100 per page is three pages");
    let unique: std::collections::HashSet<&(String, String)> = seen.iter().collect();
    assert_eq!(unique.len(), 251);
    assert_eq!(seen.len(), 251, "every thread exactly once");
}

#[tokio::test]
async fn p7_t27_snoozed_pages_by_wake_time_not_by_date() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let first = db.new_account("one@example.com", None, None).await.unwrap();
    let second = db.new_account("two@example.com", None, None).await.unwrap();
    let accounts = vec![first.id.clone(), second.id.clone()];
    // Wake order and last-message order disagree, and every wake time is
    // shared by several threads.
    for i in 0..251 {
        let owner = if i % 2 == 0 { &first.id } else { &second.id };
        let thread = format!("s-{i:03}");
        seed(
            &db,
            owner,
            &Spec {
                id: &format!("sm-{i}"),
                thread: &thread,
                at: 9_000 - i as i64,
                subject: "snoozed",
                from: ("A", "a@example.com"),
                labels: &["INBOX"],
                body: "snoozed",
                ..Default::default()
            },
        )
        .await;
        let (account, thread_id, wake) = (owner.clone(), thread.clone(), 1_000 + (i % 100) as i64);
        db.write(move |c| {
            c.execute(
                "UPDATE threads SET snoozed_until=? WHERE account_id=? AND id=?",
                rusqlite::params![wake, account, thread_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut cursor: Option<sift::search::cursor::Cursor> = None;
    let mut pages = 0;
    loop {
        let page = db
            .threads_query(ThreadsQuery {
                account_ids: accounts.clone(),
                view: View::Snoozed,
                cursor: cursor.as_ref().map(sift::search::cursor::encode),
                limit: 100,
                unread_only: false,
                has_attachment: false,
            })
            .await
            .unwrap();
        pages += 1;
        // Every page is ordered by wake time.
        let wakes: Vec<i64> = page
            .rows
            .iter()
            .map(|r| r.snoozed_until.unwrap_or(0))
            .collect();
        assert!(wakes.windows(2).all(|w| w[0] <= w[1]), "{wakes:?}");
        seen.extend(
            page.rows
                .iter()
                .map(|r| (r.account_id.clone(), r.id.clone())),
        );
        match page.next_cursor {
            Some(raw) => {
                let (sort, scope, q) = sift::db::threads::page_identity(&View::Snoozed, &accounts);
                cursor = Some(sift::search::cursor::expect(&raw, sort, &scope, &q).unwrap());
            }
            None => break,
        }
        assert!(pages < 10, "paging must terminate");
    }
    assert_eq!(pages, 3);
    let unique: std::collections::HashSet<&(String, String)> = seen.iter().collect();
    assert_eq!(unique.len(), 251);
    assert_eq!(seen.len(), 251, "every snooze exactly once");
}

#[tokio::test]
async fn p7_t28_cursors_reject_a_different_scope_query_or_sort() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("a@example.com", None, None).await.unwrap();
    let a = account.id.clone();
    for i in 0..5 {
        seed(
            &db,
            &a,
            &Spec {
                id: &format!("c-{i}"),
                thread: &format!("ct-{i}"),
                at: 100 + i,
                subject: "cursor",
                from: ("A", "a@example.com"),
                labels: &["INBOX"],
                body: "cursor",
                ..Default::default()
            },
        )
        .await;
    }
    let (sort, scope, query) =
        sift::db::threads::page_identity(&View::Inbox, std::slice::from_ref(&a));
    let good = sift::search::cursor::encode(&sift::search::cursor::Cursor {
        sort,
        scope: scope.clone(),
        query: query.clone(),
        key: (10_000, "z".into(), "z".into()),
    });
    // A well-formed cursor for this exact list is accepted.
    assert!(db
        .threads_query(ThreadsQuery {
            account_ids: vec![a.clone()],
            view: View::Inbox,
            cursor: Some(good),
            limit: 10,
            unread_only: false,
            has_attachment: false,
        })
        .await
        .is_ok());

    let cases: Vec<(&str, String)> = vec![
        ("malformed", "not-a-cursor!!".into()),
        (
            "version",
            base64_encode(
                &serde_json::json!({"v": 99, "s": "default", "c": scope, "q": query, "t": [1, "a", "b"]}),
            ),
        ),
        (
            "scope",
            base64_encode(
                &serde_json::json!({"v": 1, "s": "default", "c": "inbox|someone-else", "q": query, "t": [1, "a", "b"]}),
            ),
        ),
        (
            "sort",
            base64_encode(
                &serde_json::json!({"v": 1, "s": "snoozed", "c": scope, "q": query, "t": [1, "a", "b"]}),
            ),
        ),
        (
            "shape",
            base64_encode(
                &serde_json::json!({"v": 1, "s": "default", "c": scope, "q": query, "t": [1]}),
            ),
        ),
    ];
    for (reason, raw) in cases {
        let error = db
            .threads_query(ThreadsQuery {
                account_ids: vec![a.clone()],
                view: View::Inbox,
                cursor: Some(raw),
                limit: 10,
                unread_only: false,
                has_attachment: false,
            })
            .await
            .expect_err(reason);
        assert_eq!(error.code(), "bad_cursor", "{reason}: {error}");
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(value["detail"]["reason"], reason);
    }

    // The same cursor cannot be replayed against a different query.
    let search_scope = format!("search|{a}");
    let page = local::search_page(
        &db,
        std::slice::from_ref(&a),
        &query::parse("cursor"),
        None,
        &search_scope,
        2,
    )
    .await
    .unwrap();
    let cursor = page.next_cursor.expect("more than two matches");
    let decoded = sift::search::cursor::expect(
        &cursor,
        sift::search::cursor::SortKind::Search,
        &search_scope,
        &query::parse("cursor").fingerprint(),
    )
    .unwrap();
    let next = local::search_page(
        &db,
        std::slice::from_ref(&a),
        &query::parse("cursor"),
        Some(&decoded),
        &search_scope,
        2,
    )
    .await
    .unwrap();
    assert_eq!(next.rows.len(), 2, "the second page is real");
    let first_ids: Vec<&String> = page.rows.iter().map(|r| &r.id).collect();
    assert!(
        next.rows.iter().all(|r| !first_ids.contains(&&r.id)),
        "a page boundary must not repeat a row"
    );

    let mismatch = sift::search::cursor::expect(
        &cursor,
        sift::search::cursor::SortKind::Search,
        &search_scope,
        &query::parse("other").fingerprint(),
    )
    .expect_err("a cursor for a different query must be refused");
    assert_eq!(mismatch.code(), "bad_cursor");
    let value = serde_json::to_value(&mismatch).unwrap();
    assert_eq!(value["detail"]["reason"], "query");

    let wrong_scope = sift::search::cursor::expect(
        &cursor,
        sift::search::cursor::SortKind::Search,
        "search|someone-else",
        &query::parse("cursor").fingerprint(),
    )
    .expect_err("a cursor for a different scope must be refused");
    let value = serde_json::to_value(&wrong_scope).unwrap();
    assert_eq!(value["detail"]["reason"], "scope");
}

fn base64_encode(value: &serde_json::Value) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
}

#[tokio::test]
async fn p7_t29_query_plans_use_indexes_and_never_offset() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a, b) = fixture(dir.path()).await;
    let accounts = vec![a.clone(), b.clone()];

    let parsed = query::parse("quarterly from:ada is:read before:2030-01-01 has:attachment");
    let compiled = sift::search::compile::candidate_query(
        parsed.ast.as_ref(),
        &accounts,
        sift::search::compile::default_mailbox_scope(parsed.ast.as_ref()),
        None,
        51,
    )
    .unwrap();
    assert!(!compiled.sql.contains("OFFSET"), "{}", compiled.sql);
    let plan = explain(&db, &compiled).await;
    eprintln!("search candidate plan:\n{plan}");
    assert!(
        !plan.lines().any(|line| line.trim() == "SCAN m"),
        "the candidate query must not scan the message table:\n{plan}"
    );
    assert!(
        plan.contains("SEARCH m USING INDEX"),
        "the candidate query must reach messages through an index:\n{plan}"
    );

    let list_sql = "SELECT t.* FROM threads t WHERE t.account_id IN (?,?) AND t.in_inbox=1 AND \
         (t.last_message_at, t.account_id, t.id) < (?,?,?) \
         ORDER BY t.last_message_at DESC, t.account_id DESC, t.id DESC LIMIT ?";
    let plan = explain_raw(
        &db,
        list_sql,
        vec![
            rusqlite::types::Value::Text(a.clone()),
            rusqlite::types::Value::Text(b.clone()),
            rusqlite::types::Value::Integer(10_000),
            rusqlite::types::Value::Text("z".into()),
            rusqlite::types::Value::Text("z".into()),
            rusqlite::types::Value::Integer(101),
        ],
    )
    .await;
    eprintln!("inbox keyset plan:\n{plan}");
    assert!(
        !plan.lines().any(|line| line.trim().starts_with("SCAN t")),
        "{plan}"
    );
    assert!(plan.contains("SEARCH t USING INDEX"), "{plan}");

    let snooze_sql =
        "SELECT t.* FROM threads t WHERE t.account_id IN (?,?) AND t.snoozed_until IS NOT NULL \
         AND (t.snoozed_until, t.account_id, t.id) > (?,?,?) \
         ORDER BY t.snoozed_until ASC, t.account_id ASC, t.id ASC LIMIT ?";
    let plan = explain_raw(
        &db,
        snooze_sql,
        vec![
            rusqlite::types::Value::Text(a),
            rusqlite::types::Value::Text(b),
            rusqlite::types::Value::Integer(0),
            rusqlite::types::Value::Text(String::new()),
            rusqlite::types::Value::Text(String::new()),
            rusqlite::types::Value::Integer(101),
        ],
    )
    .await;
    eprintln!("snoozed keyset plan:\n{plan}");
    assert!(
        !plan.lines().any(|line| line.trim().starts_with("SCAN t")),
        "{plan}"
    );
}

async fn explain(db: &Db, compiled: &sift::search::compile::Compiled) -> String {
    let params: Vec<rusqlite::types::Value> = compiled
        .params
        .iter()
        .map(|p| match p {
            sift::search::compile::Param::Text(value) => {
                rusqlite::types::Value::Text(value.clone())
            }
            sift::search::compile::Param::Int(value) => rusqlite::types::Value::Integer(*value),
        })
        .collect();
    explain_raw(db, &compiled.sql, params).await
}

async fn explain_raw(db: &Db, sql: &str, params: Vec<rusqlite::types::Value>) -> String {
    let sql = format!("EXPLAIN QUERY PLAN {sql}");
    db.read(move |c| {
        let mut statement = c.prepare(&sql)?;
        let rows: Vec<String> = statement
            .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                r.get::<_, String>(3)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.join("\n"))
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn p7_t30_saved_search_matches_ad_hoc_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a, _) = fixture(dir.path()).await;
    let ad_hoc = ids(&matches(
        &db,
        std::slice::from_ref(&a),
        "label:\"Client Work\" is:read",
    )
    .await);

    let saved = db
        .saved_search_upsert(SavedSearchInput {
            id: None,
            name: "Client work".into(),
            query: "label:\"Client Work\" is:read".into(),
            account_scope: vec![a.clone()],
            sort_order: None,
        })
        .await
        .unwrap();
    assert!(saved.missing_accounts.is_empty());
    assert_eq!(saved.ast_version, query::AST_VERSION);
    // No count until one is asked for.
    let listed = db.saved_search_list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].match_count.is_none());

    // The saved mailbox resolves to exactly the ad hoc result, using only the
    // database (there is no provider and no network in this test at all).
    let parsed = query::parse(&saved.query);
    let page = local::search_page(&db, &saved.account_scope, &parsed, None, "saved", 100)
        .await
        .unwrap();
    assert_eq!(
        ids(&page
            .rows
            .iter()
            .map(|r| (r.account_id.clone(), r.id.clone()))
            .collect::<Vec<_>>()),
        ad_hoc
    );

    let count = db.saved_search_count(&saved.id).await.unwrap();
    assert_eq!(count.count, ad_hoc.len() as i64);

    // It survives a restart.
    drop(db);
    let reopened = Db::open(dir.path()).unwrap();
    let listed = reopened.saved_search_list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].query, "label:\"Client Work\" is:read");
    assert_eq!(listed[0].account_scope, vec![a]);
}

#[tokio::test]
async fn p7_t31_saved_search_edit_and_delete_never_move_mail() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a, _) = fixture(dir.path()).await;
    let fingerprint = |db: &Db| {
        let db = db.clone();
        async move {
            db.read(|c| {
                let messages: i64 =
                    c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?;
                let threads: i64 = c.query_row("SELECT count(*) FROM threads", [], |r| r.get(0))?;
                let labels: i64 =
                    c.query_row("SELECT count(*) FROM message_labels", [], |r| r.get(0))?;
                let ops: i64 = c.query_row("SELECT count(*) FROM outbox_ops", [], |r| r.get(0))?;
                Ok((messages, threads, labels, ops))
            })
            .await
            .unwrap()
        }
    };
    let before = fingerprint(&db).await;
    let saved = db
        .saved_search_upsert(SavedSearchInput {
            id: None,
            name: "Everything".into(),
            query: "quarterly".into(),
            account_scope: vec![a.clone()],
            sort_order: None,
        })
        .await
        .unwrap();
    let edited = db
        .saved_search_upsert(SavedSearchInput {
            id: Some(saved.id.clone()),
            name: "Everything else".into(),
            query: "quarterly -from:ada".into(),
            account_scope: vec![a.clone()],
            sort_order: Some(3),
        })
        .await
        .unwrap();
    assert_eq!(edited.id, saved.id);
    assert_eq!(edited.name, "Everything else");
    assert_eq!(edited.sort_order, 3);
    assert_eq!(db.saved_search_list().await.unwrap().len(), 1);
    assert_eq!(fingerprint(&db).await, before);

    db.saved_search_delete(&saved.id).await.unwrap();
    assert!(db.saved_search_list().await.unwrap().is_empty());
    assert_eq!(
        fingerprint(&db).await,
        before,
        "deleting never touches mail"
    );
}

#[tokio::test]
async fn p7_t32_a_missing_account_is_reported_never_widened() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let gone = db
        .new_account("gone@example.com", None, None)
        .await
        .unwrap();
    let alive = db
        .new_account("alive@example.com", None, None)
        .await
        .unwrap();
    for (owner, id, thread) in [
        (&gone.id, "g1", "gt"),
        (&alive.id, "s1", "st"),
        (&alive.id, "a1", "at"),
    ] {
        seed(
            &db,
            owner,
            &Spec {
                id,
                thread,
                at: 10,
                subject: "matches",
                from: ("A", "a@example.com"),
                labels: &["INBOX"],
                body: "matches",
                ..Default::default()
            },
        )
        .await;
    }
    let saved = db
        .saved_search_upsert(SavedSearchInput {
            id: None,
            name: "Scoped".into(),
            query: "matches".into(),
            account_scope: vec![gone.id.clone()],
            sort_order: None,
        })
        .await
        .unwrap();
    let removed = gone.id.clone();
    db.write(move |c| {
        c.execute(
            "DELETE FROM accounts WHERE id=?",
            rusqlite::params![removed],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let listed = db.saved_search_list().await.unwrap();
    assert_eq!(listed[0].account_scope, vec![gone.id.clone()]);
    assert_eq!(listed[0].missing_accounts, vec![gone.id.clone()]);

    // Opening it matches only that scope - never every account, even though
    // the surviving account has matching mail.
    let parsed = query::parse(&saved.query);
    let page = local::search_page(&db, &saved.account_scope, &parsed, None, "saved", 100)
        .await
        .unwrap();
    assert!(
        page.rows.is_empty(),
        "a removed account must not widen the scope to everyone else"
    );
    assert_eq!(db.saved_search_count(&saved.id).await.unwrap().count, 0);
    let everything = local::search_page(
        &db,
        std::slice::from_ref(&alive.id),
        &parsed,
        None,
        "saved",
        100,
    )
    .await
    .unwrap();
    assert_eq!(
        everything.rows.len(),
        2,
        "the surviving account still matches"
    );

    // An empty scope matches nothing rather than everything.
    let empty = db
        .saved_search_upsert(SavedSearchInput {
            id: None,
            name: "Empty".into(),
            query: "matches".into(),
            account_scope: vec![],
            sort_order: None,
        })
        .await
        .unwrap();
    let page = local::search_page(&db, &empty.account_scope, &parsed, None, "saved", 100)
        .await
        .unwrap();
    assert!(page.rows.is_empty());
}

#[tokio::test]
async fn p7_t33_saved_search_refuses_a_query_that_can_never_match() {
    let dir = tempfile::tempdir().unwrap();
    let (db, a, _) = fixture(dir.path()).await;
    for query in ["before:not-a-date", "((((", ""] {
        let error = db
            .saved_search_upsert(SavedSearchInput {
                id: None,
                name: "Broken".into(),
                query: query.into(),
                account_scope: vec![a.clone()],
                sort_order: None,
            })
            .await
            .expect_err(query);
        assert!(
            !db.saved_search_list()
                .await
                .unwrap()
                .iter()
                .any(|s| s.name == "Broken"),
            "{query} must not be stored"
        );
        let _ = error;
    }
    assert!(db.saved_search_list().await.unwrap().is_empty());
}

#[tokio::test]
async fn p7_t34_a_saved_mailbox_never_builds_a_mailbox_sized_result() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db.new_account("a@example.com", None, None).await.unwrap();
    let a = account.id.clone();
    for i in 0..1_000 {
        seed(
            &db,
            &a,
            &Spec {
                id: &format!("big-{i}"),
                thread: &format!("bt-{i}"),
                at: 1 + i as i64,
                subject: "big",
                from: ("A", "a@example.com"),
                labels: &["INBOX"],
                body: "big",
                ..Default::default()
            },
        )
        .await;
    }
    let saved = db
        .saved_search_upsert(SavedSearchInput {
            id: None,
            name: "Big".into(),
            query: "big".into(),
            account_scope: vec![a.clone()],
            sort_order: None,
        })
        .await
        .unwrap();
    let parsed = query::parse(&saved.query);
    let page = local::search_page(&db, &saved.account_scope, &parsed, None, "saved", 100)
        .await
        .unwrap();
    assert_eq!(page.rows.len(), 100, "one page, never the whole mailbox");
    let count = db.saved_search_count(&saved.id).await.unwrap();
    assert_eq!(count.count, 1_000);

    // The candidate query is one statement, applies the limit in SQL, and
    // never offsets into the mailbox.
    let compiled = sift::search::compile::candidate_query(
        parsed.ast.as_ref(),
        &saved.account_scope,
        true,
        None,
        101,
    )
    .unwrap();
    assert!(compiled.sql.starts_with("WITH matched AS (SELECT"));
    assert!(!compiled.sql.contains("OFFSET"));
    let counted =
        sift::search::compile::count_query(parsed.ast.as_ref(), &saved.account_scope, true)
            .unwrap();
    assert!(counted.sql.starts_with("WITH matched AS (SELECT"));
    assert!(counted.sql.ends_with("SELECT COUNT(*) FROM matched"));
}
