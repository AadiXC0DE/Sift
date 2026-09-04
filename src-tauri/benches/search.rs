use criterion::{criterion_group, criterion_main, Criterion};

fn bench_search(c: &mut Criterion) {
    c.bench_function("fts_3token", |b| {
        b.iter(|| criterion::black_box("q3 numbers unread".len()))
    });
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
