use criterion::{black_box, criterion_group, criterion_main, Criterion};
use qc_model::object::{ScoreBreakdown, ScoredObject};
use qc_model::scenario::CapacityConstraint;
use qc_solver::greedy::GreedySolver;
use qc_solver::solver::Solver;

fn make_objects(n: usize) -> Vec<ScoredObject> {
    (0..n)
        .map(|i| {
            let size = ((i * 7 + 13) % 100_000 + 100) as u64;
            let benefit = (i as f64 * 0.1 + 1.0) / (1.0 + (i as f64 / n as f64));
            ScoredObject {
                object_id: format!("obj-{i:06}"),
                cache_key: format!("/content/{i:06}"),
                size_bytes: size,
                net_benefit: benefit,
                score_breakdown: ScoreBreakdown {
                    expected_hit_benefit: benefit,
                    freshness_cost: 0.0,
                    net_benefit: benefit,
                    capacity_shadow_cost: None,
                },
            }
        })
        .collect()
}

fn bench_greedy_10k(c: &mut Criterion) {
    let objects = make_objects(10_000);
    let total_size: u64 = objects.iter().map(|o| o.size_bytes).sum();
    let constraint = CapacityConstraint {
        capacity_bytes: total_size / 3,
    };

    c.bench_function("greedy_solver_10k", |b| {
        b.iter(|| {
            let result = GreedySolver
                .solve(black_box(&objects), black_box(&constraint))
                .unwrap();
            black_box(result);
        })
    });
}

criterion_group!(benches, bench_greedy_10k);
criterion_main!(benches);
