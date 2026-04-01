/// Wall-clock performance guard to catch regressions.
/// Run with: cargo test --release -p qc-solver -- --ignored
use std::time::Instant;

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

#[test]
#[ignore]
fn greedy_10k_under_1_second() {
    let objects = make_objects(10_000);
    let total: u64 = objects.iter().map(|o| o.size_bytes).sum();
    let constraint = CapacityConstraint {
        capacity_bytes: total / 3,
    };

    let start = Instant::now();
    for _ in 0..100 {
        let _ = GreedySolver.solve(&objects, &constraint).unwrap();
    }
    let elapsed = start.elapsed();
    let per_run_ms = elapsed.as_millis() as f64 / 100.0;

    eprintln!(
        "greedy_10k: {per_run_ms:.1}ms per run ({} runs in {:?})",
        100, elapsed
    );
    assert!(
        per_run_ms < 1000.0,
        "greedy_10k took {per_run_ms:.1}ms, exceeds 1000ms target"
    );
}
