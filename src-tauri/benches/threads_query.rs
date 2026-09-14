//! P7.3 benchmark: thread-list paging over 100 000 messages.
//!
//! Each page is a keyset over the same tuple the ORDER BY uses, so the first
//! page and a deep page must cost the same. Run with
//! `cargo bench --bench threads_query -- --nocapture` to print the plans.

#[path = "support/mod.rs"]
mod support;

use criterion::{criterion_group, criterion_main, Criterion};
use sift::dto::{ThreadsQuery, View};
use sift::search::cursor;

fn bench_threads_query(c: &mut Criterion) {
    let fixture = support::seed();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let accounts = fixture.accounts.clone();

    let (sort, scope, query) = sift::db::threads::page_identity(&View::Inbox, &accounts);
    support::report_plan(
        &runtime,
        &fixture.db,
        "inbox page (keyset)",
        "SELECT t.* FROM threads t WHERE t.account_id IN ('a','b') AND t.in_inbox=1 \
         AND (t.last_message_at, t.account_id, t.id) < (1, 'z', 'z') \
         ORDER BY t.last_message_at DESC, t.account_id DESC, t.id DESC LIMIT 101",
    );

    let inbox = ThreadsQuery {
        account_ids: accounts.clone(),
        view: View::Inbox,
        cursor: None,
        limit: 100,
        unread_only: false,
        has_attachment: false,
    };
    c.bench_function("threads_inbox_page_100k", |b| {
        b.iter(|| {
            runtime.block_on(async {
                fixture
                    .db
                    .threads_query(inbox.clone())
                    .await
                    .unwrap()
                    .rows
                    .len()
            })
        })
    });

    let snoozed = ThreadsQuery {
        view: View::Snoozed,
        ..inbox.clone()
    };
    c.bench_function("threads_snoozed_page_100k", |b| {
        b.iter(|| {
            runtime.block_on(async {
                fixture
                    .db
                    .threads_query(snoozed.clone())
                    .await
                    .unwrap()
                    .rows
                    .len()
            })
        })
    });

    // Walk 20 pages so the deep page is a real keyset position, not page one.
    let mut cursor: Option<String> = None;
    for _ in 0..20 {
        let page = runtime
            .block_on(fixture.db.threads_query(ThreadsQuery {
                cursor: cursor.clone(),
                ..inbox.clone()
            }))
            .unwrap();
        cursor = page
            .next_cursor
            .map(|raw| cursor::expect(&raw, sort, &scope, &query).unwrap().key)
            .map(|key| {
                cursor::encode(&cursor::Cursor {
                    sort,
                    scope: scope.clone(),
                    query: query.clone(),
                    key,
                })
            });
        if cursor.is_none() {
            break;
        }
    }
    let deep = cursor.clone();
    c.bench_function("threads_inbox_deep_page_100k", |b| {
        b.iter(|| {
            runtime.block_on(async {
                fixture
                    .db
                    .threads_query(ThreadsQuery {
                        cursor: deep.clone(),
                        ..inbox.clone()
                    })
                    .await
                    .unwrap()
                    .rows
                    .len()
            })
        })
    });
}

criterion_group!(benches, bench_threads_query);
criterion_main!(benches);
