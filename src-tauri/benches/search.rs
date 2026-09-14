//! P7.3 benchmark: one page of a local search over 100 000 messages.
//!
//! The candidate query must apply every filter before LIMIT and never offset
//! into the mailbox, so the cost of a page is set by the filters and the
//! index, not by how far the user has scrolled. Run with:
//! `cargo bench --bench search -- --nocapture` to print the query plan.

#[path = "support/mod.rs"]
mod support;

use criterion::{criterion_group, criterion_main, Criterion};
use sift::search::{local, query};

fn bench_search(c: &mut Criterion) {
    let fixture = support::seed();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let accounts = fixture.accounts.clone();

    let plain = query::parse("quarterly");
    support::report_plan(
        &runtime,
        &fixture.db,
        "search candidate (plain term)",
        &sift::search::compile::candidate_query(plain.ast.as_ref(), &accounts, true, None, 101)
            .unwrap()
            .sql
            .clone(),
    );
    let filtered = query::parse("quarterly from:ada is:unread has:attachment after:2023-01-01");
    support::report_plan(
        &runtime,
        &fixture.db,
        "search candidate (filtered)",
        &sift::search::compile::candidate_query(filtered.ast.as_ref(), &accounts, true, None, 101)
            .unwrap()
            .sql
            .clone(),
    );

    c.bench_function("search_page_100k_plain", |b| {
        b.iter(|| {
            runtime.block_on(async {
                local::search_page(&fixture.db, &accounts, &plain, None, "bench", 100)
                    .await
                    .unwrap()
                    .rows
                    .len()
            })
        })
    });

    c.bench_function("search_page_100k_filtered", |b| {
        b.iter(|| {
            runtime.block_on(async {
                local::search_page(&fixture.db, &accounts, &filtered, None, "bench", 100)
                    .await
                    .unwrap()
                    .rows
                    .len()
            })
        })
    });

    // A deep page must cost about the same as the first: it is a keyset, not
    // an offset.
    let first = runtime
        .block_on(local::search_page(
            &fixture.db,
            &accounts,
            &plain,
            None,
            "bench",
            100,
        ))
        .unwrap();
    let mut cursor = first.next_cursor.as_deref().map(|raw| {
        sift::search::cursor::expect(
            raw,
            sift::search::cursor::SortKind::Search,
            "bench",
            &plain.fingerprint(),
        )
        .unwrap()
    });
    for _ in 0..20 {
        let page = runtime
            .block_on(local::search_page(
                &fixture.db,
                &accounts,
                &plain,
                cursor.as_ref(),
                "bench",
                100,
            ))
            .unwrap();
        cursor = page.next_cursor.as_deref().map(|raw| {
            sift::search::cursor::expect(
                raw,
                sift::search::cursor::SortKind::Search,
                "bench",
                &plain.fingerprint(),
            )
            .unwrap()
        });
        if cursor.is_none() {
            break;
        }
    }
    let deep = cursor.clone();
    c.bench_function("search_page_100k_deep_cursor", |b| {
        b.iter(|| {
            runtime.block_on(async {
                local::search_page(&fixture.db, &accounts, &plain, deep.as_ref(), "bench", 100)
                    .await
                    .unwrap()
                    .rows
                    .len()
            })
        })
    });

    let saved = runtime.block_on(async {
        fixture
            .db
            .saved_search_upsert(sift::dto::SavedSearchInput {
                id: None,
                name: "Bench".into(),
                query: "quarterly".into(),
                account_scope: accounts.clone(),
                sort_order: None,
            })
            .await
            .unwrap()
    });
    c.bench_function("saved_search_count_100k", |b| {
        b.iter(|| {
            runtime.block_on(async {
                fixture
                    .db
                    .saved_search_count(&saved.id)
                    .await
                    .unwrap()
                    .count
            })
        })
    });
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
