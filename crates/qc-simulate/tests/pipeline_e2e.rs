/// End-to-end pipeline test: generate → aggregate → score → solve → replay.
/// Validates the full data flow produces consistent results.
use qc_model::scenario::{
    CapacityConstraint, FreshnessModel, ScenarioConfig, ScoringVersion, StaleCostOverrides,
    StalePenaltyClass, StalePenaltyConfig,
};
use qc_simulate::baselines::StaticPolicy;
use qc_simulate::engine::{ReplayEconConfig, TraceReplayEngine};
use qc_simulate::synthetic::{self, SyntheticConfig};
use qc_solver::greedy::GreedySolver;
use qc_solver::score::BenefitCalculator;
use qc_solver::solver::Solver;

fn pipeline_config() -> ScenarioConfig {
    ScenarioConfig {
        capacity_bytes: 500_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.0001,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::Low,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
        scoring_version: ScoringVersion::V1Frequency,
    }
}

#[test]
fn full_pipeline_produces_consistent_results() {
    // 1. Generate
    let syn_config = SyntheticConfig {
        num_objects: 200,
        num_requests: 10_000,
        zipf_alpha: 0.8,
        seed: 123,
        ..SyntheticConfig::default()
    };
    let events = synthetic::generate(&syn_config).unwrap();
    assert_eq!(events.len(), 10_000);

    // 2. Aggregate features
    let config = pipeline_config();
    let features = synthetic::aggregate_features(&events, config.time_window_seconds);
    assert!(!features.is_empty());
    assert!(features.len() <= 200);

    // 3. Score
    let scored = BenefitCalculator::score_all(&features, &config).unwrap();
    assert_eq!(scored.len(), features.len());
    // At least some objects should have positive benefit
    let positive_count = scored.iter().filter(|s| s.net_benefit > 0.0).count();
    assert!(
        positive_count > 0,
        "some objects should have positive benefit"
    );

    // 4. Solve
    let constraint = CapacityConstraint {
        capacity_bytes: config.capacity_bytes,
    };
    let result = GreedySolver.solve(&scored, &constraint).unwrap();
    assert!(result.objective_value > 0.0, "objective should be positive");
    assert!(result.feasible);

    // 5. Replay
    let cached_keys = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.clone());
    let mut policy = StaticPolicy::new(cached_keys);

    let default_class = StalePenaltyClass::Low;
    let econ =
        ReplayEconConfig::from_features(&features, config.latency_value_per_ms, default_class);
    let metrics = TraceReplayEngine::replay_with_econ(&events, &mut policy, &econ).unwrap();

    // Verify replay metrics are consistent
    assert!(metrics.hit_ratio > 0.0, "should have cache hits");
    assert!(metrics.hit_ratio <= 1.0);
    assert_eq!(metrics.total_requests, 10_000);
    assert!(
        metrics.estimated_cost_savings > 0.0,
        "should save on origin costs"
    );
    assert!(
        metrics.policy_objective_value.is_finite(),
        "objective should be finite"
    );
}

#[test]
fn v2_pipeline_produces_results() {
    let syn_config = SyntheticConfig {
        num_objects: 100,
        num_requests: 5_000,
        zipf_alpha: 0.8,
        seed: 456,
        ..SyntheticConfig::default()
    };
    let events = synthetic::generate(&syn_config).unwrap();

    let mut config = pipeline_config();
    config.scoring_version = ScoringVersion::V2ReuseDistance;

    // aggregate_features with reuse distance
    let features = synthetic::aggregate_features(&events, config.time_window_seconds);
    let scored = BenefitCalculator::score_all(&features, &config).unwrap();

    let constraint = CapacityConstraint {
        capacity_bytes: config.capacity_bytes,
    };
    let result = GreedySolver.solve(&scored, &constraint).unwrap();

    assert!(result.feasible);
    // V2 may score differently than V1 but should still produce a valid result
    assert!(result.objective_value.is_finite());
}
