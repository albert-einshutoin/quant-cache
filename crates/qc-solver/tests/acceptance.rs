/// Phase 7 Acceptance: Optimality gap over 50 synthetic cases.
/// Target: median < 5%, p95 < 10% (n <= 1,000).
use qc_model::scenario::{
    CapacityConstraint, FreshnessModel, ScenarioConfig, StaleCostOverrides, StalePenaltyClass,
    StalePenaltyConfig,
};
use qc_simulate::synthetic::{self, SyntheticConfig};
use qc_solver::greedy::GreedySolver;
use qc_solver::ilp::ExactIlpSolver;
use qc_solver::score::BenefitCalculator;
use qc_solver::solver::Solver;

#[test]
#[ignore] // slow — run with: cargo test --release -p qc-solver -- --ignored
fn optimality_gap_50_cases() {
    let config = ScenarioConfig {
        capacity_bytes: 500_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.00005,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::High,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
    };

    let mut gaps: Vec<f64> = Vec::with_capacity(50);

    for seed in 0u64..50 {
        let syn_config = SyntheticConfig {
            num_objects: 1_000,
            num_requests: 50_000,
            seed,
            ..SyntheticConfig::default()
        };

        let events = synthetic::generate(&syn_config).unwrap();
        let features = synthetic::aggregate_features(&events, config.time_window_seconds);
        let scored = BenefitCalculator::score_all(&features, &config).unwrap();

        let constraint = CapacityConstraint {
            capacity_bytes: config.capacity_bytes,
        };

        let greedy = GreedySolver.solve(&scored, &constraint).unwrap();
        let ilp = ExactIlpSolver.solve(&scored, &constraint).unwrap();

        let gap = if ilp.objective_value > 1e-12 {
            (ilp.objective_value - greedy.objective_value) / ilp.objective_value
        } else {
            0.0
        };

        assert!(
            gap >= -1e-4,
            "seed {seed}: ILP ({}) < Greedy ({}) by more than rounding tolerance",
            ilp.objective_value,
            greedy.objective_value
        );

        gaps.push(gap);
    }

    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let median = gaps[24];
    let p95 = gaps[47]; // 95th percentile of 50 samples

    eprintln!("Optimality gap over 50 cases:");
    eprintln!("  min:    {:.4}%", gaps[0] * 100.0);
    eprintln!("  median: {:.4}%", median * 100.0);
    eprintln!("  p95:    {:.4}%", p95 * 100.0);
    eprintln!("  max:    {:.4}%", gaps[49] * 100.0);

    assert!(
        median < 0.05,
        "median gap {:.4}% exceeds 5% target",
        median * 100.0
    );
    assert!(p95 < 0.10, "p95 gap {:.4}% exceeds 10% target", p95 * 100.0);
}
