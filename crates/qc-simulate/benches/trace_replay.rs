use criterion::{black_box, criterion_group, criterion_main, Criterion};
use qc_model::compact_trace::CompactTraceEvent;
use qc_simulate::baselines::{GdsfPolicy, LruPolicy};
use qc_simulate::compact_baselines::CompactLruPolicy;
use qc_simulate::engine::{CompactReplayEconConfig, ReplayEconConfig, TraceReplayEngine};
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

    c.bench_function("lru_replay_1m_string", |b| {
        b.iter(|| {
            let mut policy = LruPolicy::new(50_000_000);
            let metrics = TraceReplayEngine::replay(black_box(&events), &mut policy).unwrap();
            black_box(metrics);
        })
    });
}

fn bench_lru_replay_1m_compact(c: &mut Criterion) {
    let events = generate_trace(1_000_000);
    let (compact_events, mut interner) = CompactTraceEvent::intern_batch(&events);
    let econ =
        CompactReplayEconConfig::from_econ_config(&ReplayEconConfig::default(), &mut interner);

    c.bench_function("lru_replay_1m_compact", |b| {
        b.iter(|| {
            let mut policy = CompactLruPolicy::new(50_000_000);
            let metrics = TraceReplayEngine::replay_compact_with_econ(
                black_box(&compact_events),
                &mut policy,
                &econ,
            )
            .unwrap();
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

fn bench_intern_batch_1m(c: &mut Criterion) {
    let events = generate_trace(1_000_000);

    c.bench_function("intern_batch_1m", |b| {
        b.iter(|| {
            let (compact, interner) = CompactTraceEvent::intern_batch(black_box(&events));
            black_box((compact, interner));
        })
    });
}

fn bench_memory_comparison(c: &mut Criterion) {
    let events = generate_trace(100_000);

    c.bench_function("memory_string_100k", |b| {
        b.iter(|| {
            let e = events.clone();
            black_box(e.len());
        })
    });

    let (compact_events, interner) = CompactTraceEvent::intern_batch(&events);

    c.bench_function("memory_compact_100k", |b| {
        b.iter(|| {
            let e = compact_events.clone();
            let i = interner.clone();
            black_box((e.len(), i.len()));
        })
    });
}

criterion_group!(
    benches,
    bench_lru_replay_1m,
    bench_lru_replay_1m_compact,
    bench_gdsf_replay_1m,
    bench_intern_batch_1m,
    bench_memory_comparison
);
criterion_main!(benches);
