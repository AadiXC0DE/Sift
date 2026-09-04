use criterion::{criterion_group, criterion_main, Criterion};

fn bench_threads_query(c: &mut Criterion) {
    c.bench_function("inbox_page_100", |b| {
        b.iter(|| {
            // micro-bench placeholder: JSON round-trip cost of a page
            let rows: Vec<u8> = vec![0u8; 60 * 100];
            criterion::black_box(rows.len())
        })
    });
}

criterion_group!(benches, bench_threads_query);
criterion_main!(benches);
