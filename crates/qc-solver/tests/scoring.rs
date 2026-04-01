use qc_model::object::ObjectFeatures;
use qc_model::scenario::{
    FreshnessModel, ScenarioConfig, StaleCostOverrides, StalePenaltyClass, StalePenaltyConfig,
};
use qc_solver::score::BenefitCalculator;

fn make_object(id: &str, size: u64, requests: u64, eligible: bool) -> ObjectFeatures {
    ObjectFeatures {
        object_id: id.into(),
        cache_key: format!("/{id}"),
        size_bytes: size,
        eligible_for_cache: eligible,
        request_count: requests,
        request_rate: requests as f64 / 86400.0,
        avg_response_bytes: size,
        avg_origin_cost: 0.003,
        avg_latency_saving_ms: 50.0,
        ttl_seconds: 3600,
        update_rate: 0.001,
        last_modified: None,
        stale_penalty_class: StalePenaltyClass::Medium,
        purge_group: None,
        origin_group: None,
        mean_reuse_distance: None,
        reuse_distance_p50: None,
        reuse_distance_p95: None,
    }
}

fn ttl_only_config() -> ScenarioConfig {
    ScenarioConfig {
        capacity_bytes: 10_000_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.00005,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::Medium,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
    }
}

fn invalidation_config() -> ScenarioConfig {
    ScenarioConfig {
        capacity_bytes: 10_000_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.0001,
        freshness_model: FreshnessModel::InvalidationOnUpdate {
            invalidation_cost: 0.001,
        },
    }
}

// ── BenefitCalculator ───────────────────────────────────────────────

#[test]
fn ineligible_object_scores_zero() {
    let obj = make_object("ineligible", 1024, 1000, false);
    let scored = BenefitCalculator::score(&obj, &ttl_only_config()).unwrap();
    assert_eq!(scored.net_benefit, 0.0);
    assert_eq!(scored.score_breakdown.expected_hit_benefit, 0.0);
    assert_eq!(scored.score_breakdown.freshness_cost, 0.0);
}

#[test]
fn ttl_only_scoring_positive_benefit() {
    let obj = make_object("popular", 8192, 5000, true);
    let scored = BenefitCalculator::score(&obj, &ttl_only_config()).unwrap();

    assert!(
        scored.score_breakdown.expected_hit_benefit > 0.0,
        "expected positive hit benefit"
    );
    assert!(
        scored.score_breakdown.freshness_cost > 0.0,
        "expected non-zero freshness cost with update_rate > 0"
    );
    assert_eq!(
        scored.net_benefit,
        scored.score_breakdown.expected_hit_benefit - scored.score_breakdown.freshness_cost
    );
}

#[test]
fn ttl_only_scoring_formula_check() {
    let obj = make_object("check", 1024, 1000, true);
    let config = ttl_only_config();
    let scored = BenefitCalculator::score(&obj, &config).unwrap();

    // expected_hit_benefit = requests * (latency_saving * latency_value + origin_cost)
    let expected_hit = 1000.0 * (50.0 * 0.00005 + 0.003);
    assert!(
        (scored.score_breakdown.expected_hit_benefit - expected_hit).abs() < 1e-9,
        "hit benefit: got {}, expected {}",
        scored.score_breakdown.expected_hit_benefit,
        expected_hit
    );

    // freshness_cost = requests * p_stale * penalty
    let p_stale = 1.0 - (-0.001_f64 * 3600.0).exp();
    let penalty = StalePenaltyClass::Medium.to_cost();
    let expected_freshness = 1000.0 * p_stale * penalty;
    assert!(
        (scored.score_breakdown.freshness_cost - expected_freshness).abs() < 1e-9,
        "freshness cost: got {}, expected {}",
        scored.score_breakdown.freshness_cost,
        expected_freshness
    );
}

#[test]
fn invalidation_scoring_formula_check() {
    let obj = make_object("inv", 2048, 2000, true);
    let config = invalidation_config();
    let scored = BenefitCalculator::score(&obj, &config).unwrap();

    // expected_hit_benefit = requests * (latency_saving * latency_value + origin_cost)
    let expected_hit = 2000.0 * (50.0 * 0.0001 + 0.003);
    assert!(
        (scored.score_breakdown.expected_hit_benefit - expected_hit).abs() < 1e-9,
        "hit benefit: got {}, expected {}",
        scored.score_breakdown.expected_hit_benefit,
        expected_hit
    );

    // freshness_cost = update_rate * time_window * invalidation_cost
    let expected_freshness = 0.001 * 86400.0 * 0.001;
    assert!(
        (scored.score_breakdown.freshness_cost - expected_freshness).abs() < 1e-9,
        "freshness cost: got {}, expected {}",
        scored.score_breakdown.freshness_cost,
        expected_freshness
    );
}

#[test]
fn score_all_processes_multiple_objects() {
    let objects = vec![
        make_object("a", 1024, 100, true),
        make_object("b", 2048, 200, true),
        make_object("c", 512, 50, false),
    ];
    let scored = BenefitCalculator::score_all(&objects, &ttl_only_config()).unwrap();
    assert_eq!(scored.len(), 3);
    // Eligible objects get scored (may be positive or negative depending on params)
    assert_ne!(scored[0].net_benefit, 0.0);
    assert_ne!(scored[1].net_benefit, 0.0);
    assert_eq!(scored[2].net_benefit, 0.0); // ineligible
}

#[test]
fn higher_request_count_yields_higher_benefit() {
    // Use a static object (update_rate=0) so freshness cost doesn't dominate
    let config = ttl_only_config();
    let mut low = make_object("low", 1024, 100, true);
    low.update_rate = 0.0;
    let mut high = make_object("high", 1024, 10000, true);
    high.update_rate = 0.0;
    let low_scored = BenefitCalculator::score(&low, &config).unwrap();
    let high_scored = BenefitCalculator::score(&high, &config).unwrap();
    assert!(
        high_scored.net_benefit > low_scored.net_benefit,
        "more requests should yield higher benefit"
    );
}

#[test]
fn zero_update_rate_means_no_freshness_cost_ttl() {
    let mut obj = make_object("static", 1024, 1000, true);
    obj.update_rate = 0.0;
    let scored = BenefitCalculator::score(&obj, &ttl_only_config()).unwrap();
    assert_eq!(scored.score_breakdown.freshness_cost, 0.0);
}

#[test]
fn zero_update_rate_means_no_freshness_cost_invalidation() {
    let mut obj = make_object("static", 1024, 1000, true);
    obj.update_rate = 0.0;
    let scored = BenefitCalculator::score(&obj, &invalidation_config()).unwrap();
    assert_eq!(scored.score_breakdown.freshness_cost, 0.0);
}
