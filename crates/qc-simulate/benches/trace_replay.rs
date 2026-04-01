use criterion::{black_box, criterion_group, criterion_main, Criterion};
use qc_simulate::baselines::{GdsfPolicy, LruPolicy};
use qc_simulate::engine::TraceReplayEngine;
use qc_simulate::synthetic::{self, SyntheticConfig};

fn generate_trace(n: usize) -> Vec<qc_model::trace::RequestTraceEvent> {
    let config = SyntheticConfig {
        num_objects: 1_000,
        num_requests: n,
        seed: 42,
        ..SyntheticConfig::default()
    };
    synthetic::generate(&config).unwrap()
}

fn bench_lru_replay_1m(c: &mut Criterion) {
    let events = generate_trace(1_000_000);

    c.bench_function("lru_replay_1m", |b| {
        b.iter(|| {
            let mut policy = LruPolicy::new(50_000_000);
            let metrics = TraceReplayEngine::replay(black_box(&events), &mut policy).unwrap();
            black_box(metrics);
        })
    });
}

fn bench_gdsf_replay_1m(c: &mut Criterion) {
    let events = generate_trace(1_000_000);

    c.bench_function("gdsf_replay_1m", |b| {
        b.iter(|| {
            let mut policy = GdsfPolicy::new(50_000_000);
            let metrics = TraceReplayEngine::replay(black_box(&events), &mut policy).unwrap();
            black_box(metrics);
        })
    });
}

criterion_group!(benches, bench_lru_replay_1m, bench_gdsf_replay_1m);
criterion_main!(benches);
