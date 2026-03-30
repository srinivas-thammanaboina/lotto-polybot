use criterion::{Criterion, criterion_group, criterion_main};

fn bench_placeholder(c: &mut Criterion) {
    c.bench_function("placeholder", |b| {
        b.iter(|| {
            // Will benchmark full hot-path latency
            std::hint::black_box(1 + 1)
        })
    });
}

criterion_group!(benches, bench_placeholder);
criterion_main!(benches);
