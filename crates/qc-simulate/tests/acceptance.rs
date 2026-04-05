/// Phase 7 Acceptance: baseline comparison tests.
///
/// V1 requirements (from v1-design.md §10.4):
/// - "定量比較が可能であること（常に勝つことは V1 要件ではない）"
/// - EconomicGreedy should be competitive with GDSF (not always better, since
///   static offline policy vs online adaptive has structural disadvantage)
use qc_model::scenario::{
    CapacityConstraint, FreshnessModel, ScenarioConfig, ScoringVersion, StaleCostOverrides,
    StalePenaltyClass, StalePenaltyConfig,
};
use qc_simulate::baselines::{GdsfPolicy, LruPolicy, StaticPolicy};
use qc_simulate::comparator::Comparator;
use qc_simulate::engine::{CachePolicy, ReplayEconConfig};
use qc_simulate::synthetic::{self, SyntheticConfig};
use qc_solver::greedy::GreedySolver;
use qc_solver::score::BenefitCalculator;
use qc_solver::solver::Solver;

fn make_econ(
    features: &[qc_model::object::ObjectFeatures],
    config: &ScenarioConfig,
) -> ReplayEconConfig {
    let default_class = match &config.freshness_model {
        FreshnessModel::TtlOnly { stale_penalty } => stale_penalty.default_class,
        FreshnessModel::InvalidationOnUpdate { .. } => StalePenaltyClass::None,
    };
    ReplayEconConfig::from_features(features, config.latency_value_per_ms, default_class)
}

fn run_comparison(seed: u64, config: &ScenarioConfig) -> (f64, f64, f64, f64, f64, f64) {
    let syn_config = SyntheticConfig {
        num_objects: 1_000,
        num_requests: 100_000,
        zipf_alpha: 0.8,
        seed,
        ..SyntheticConfig::default()
    };

    let events = synthetic::generate(&syn_config).unwrap();

    let features = synthetic::aggregate_features(&events, config.time_window_seconds);
    let econ = make_econ(&features, config);
    let scored = BenefitCalculator::score_all(&features, config).unwrap();
    let constraint = CapacityConstraint {
        capacity_bytes: config.capacity_bytes,
    };
    let result = GreedySolver.solve(&scored, &constraint).unwrap();
    let cached_keys = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.clone());

    let mut lru = LruPolicy::new(config.capacity_bytes);
    let mut gdsf = GdsfPolicy::new(config.capacity_bytes);
    let mut economic = StaticPolicy::new(cached_keys);

    let mut policies: Vec<&mut dyn CachePolicy> = vec![&mut lru, &mut gdsf, &mut economic];

    let report = Comparator::compare_with_econ(&events, &mut policies, &econ).unwrap();

    (
        report.results[0].metrics.policy_objective_value, // LRU obj
        report.results[1].metrics.policy_objective_value, // GDSF obj
        report.results[2].metrics.policy_objective_value, // Econ obj
        report.results[0].metrics.estimated_cost_savings, // LRU savings
        report.results[1].metrics.estimated_cost_savings, // GDSF savings
        report.results[2].metrics.estimated_cost_savings, // Econ savings
    )
}

/// EconomicGreedy should beat LRU on cost savings in most cases.
#[test]
#[ignore]
fn economic_greedy_competitive_with_lru_on_cost_savings() {
    let config = ScenarioConfig {
        capacity_bytes: 5_000_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.00005,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::High,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
        scoring_version: ScoringVersion::default(),
    };

    let mut econ_wins_savings = 0;
    for seed in 0u64..20 {
        let (_, _, _, lru_s, _, econ_s) = run_comparison(seed, &config);
        if econ_s >= lru_s {
            econ_wins_savings += 1;
        }
        eprintln!("seed {seed:2}: LRU=${lru_s:.2} Econ=${econ_s:.2}");
    }

    eprintln!("EconomicGreedy beats LRU on cost savings: {econ_wins_savings}/20");
    assert!(
        econ_wins_savings >= 10,
        "EconomicGreedy should beat LRU on cost savings in at least half of cases"
    );
}

/// All three policies produce non-zero metrics (comparison is functional).
#[test]
#[ignore]
fn comparison_produces_meaningful_metrics() {
    let config = ScenarioConfig {
        capacity_bytes: 5_000_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.00005,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::High,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
        scoring_version: ScoringVersion::default(),
    };

    let (lru_obj, gdsf_obj, econ_obj, lru_s, gdsf_s, econ_s) = run_comparison(42, &config);

    eprintln!("Objective: LRU={lru_obj:.2} GDSF={gdsf_obj:.2} Econ={econ_obj:.2}");
    eprintln!("Savings:   LRU={lru_s:.2} GDSF={gdsf_s:.2} Econ={econ_s:.2}");

    // All policies produce positive cost savings (origin cost avoided)
    assert!(lru_s > 0.0, "LRU savings should be positive");
    assert!(gdsf_s > 0.0, "GDSF savings should be positive");
    assert!(econ_s > 0.0, "Econ savings should be positive");

    // Objective may be negative when stale penalties outweigh hit benefits
    // (correct behavior for high-update-rate objects with high stale penalty).
    // We verify that values are finite and that the comparison is functional.
    assert!(lru_obj.is_finite(), "LRU objective should be finite");
    assert!(gdsf_obj.is_finite(), "GDSF objective should be finite");
    assert!(econ_obj.is_finite(), "Econ objective should be finite");

    // EconomicGreedy cost savings should not be pathologically low
    assert!(
        econ_s > lru_s * 0.5,
        "Econ savings ({econ_s:.2}) should be within 2x of LRU ({lru_s:.2})"
    );
}
