use qc_model::object::ObjectFeatures;
use qc_model::scenario::{
    FreshnessModel, ScenarioConfig, ScoringVersion, StaleCostOverrides, StalePenaltyClass,
    StalePenaltyConfig,
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
        scoring_version: ScoringVersion::V1Frequency,
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
        scoring_version: ScoringVersion::V1Frequency,
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

// ── V2 Reuse-Distance Scoring ──────────────────────────────────────

fn v2_config() -> ScenarioConfig {
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
        scoring_version: ScoringVersion::V2ReuseDistance,
    }
}

#[test]
fn v2_falls_back_to_v1_when_no_reuse_distance() {
    let obj = make_object("no-rd", 1024, 1000, true);
    // obj has mean_reuse_distance = None
    let v1_scored = BenefitCalculator::score(&obj, &ttl_only_config()).unwrap();
    let v2_scored = BenefitCalculator::score(&obj, &v2_config()).unwrap();
    assert!(
        (v1_scored.net_benefit - v2_scored.net_benefit).abs() < 1e-9,
        "V2 should fall back to V1 when reuse distance is None"
    );
}

#[test]
fn v2_low_reuse_distance_scores_near_v1() {
    // Object with very low reuse distance (high locality) should score close to V1
    let mut obj = make_object("hot", 1024, 1000, true);
    obj.update_rate = 0.0; // simplify: no freshness cost
    obj.mean_reuse_distance = Some(1.0);
    obj.reuse_distance_p50 = Some(1.0);
    obj.reuse_distance_p95 = Some(5.0);

    let v1_scored = BenefitCalculator::score(&obj, &ttl_only_config()).unwrap();
    let v2_scored = BenefitCalculator::score(&obj, &v2_config()).unwrap();

    // cache can hold 10_000_000 / 1024 ≈ 9765 objects
    // p_hit = exp(-1.0 / 9765) ≈ 0.9999
    // V2 should be very close to V1
    let ratio = v2_scored.net_benefit / v1_scored.net_benefit;
    assert!(
        ratio > 0.99,
        "low reuse distance should score near V1: ratio={ratio}"
    );
}

#[test]
fn v2_high_reuse_distance_scores_lower() {
    // Object with very high reuse distance (bad locality) should score much lower
    let config = v2_config();
    let mut obj = make_object("cold", 1024, 1000, true);
    obj.update_rate = 0.0;
    obj.mean_reuse_distance = Some(50000.0);
    obj.reuse_distance_p50 = Some(50000.0);
    obj.reuse_distance_p95 = Some(100000.0);

    let v1_scored = BenefitCalculator::score(&obj, &ttl_only_config()).unwrap();
    let v2_scored = BenefitCalculator::score(&obj, &config).unwrap();

    // cache can hold ~9765 objects, rd_p50=50000 >> cache size
    // p_hit = exp(-50000/9765) ≈ 0.006 → big reduction
    assert!(
        v2_scored.net_benefit < v1_scored.net_benefit * 0.1,
        "high reuse distance should significantly reduce benefit: v1={}, v2={}",
        v1_scored.net_benefit,
        v2_scored.net_benefit
    );
}

#[test]
fn v2_reuse_distance_discriminates_hot_vs_cold() {
    // Same request_count but different reuse distances → V2 ranks differently
    let config = v2_config();

    let mut hot = make_object("hot", 1024, 1000, true);
    hot.update_rate = 0.0;
    hot.mean_reuse_distance = Some(10.0);
    hot.reuse_distance_p50 = Some(10.0);
    hot.reuse_distance_p95 = Some(50.0);

    let mut cold = make_object("cold", 1024, 1000, true);
    cold.update_rate = 0.0;
    cold.mean_reuse_distance = Some(20000.0);
    cold.reuse_distance_p50 = Some(20000.0);
    cold.reuse_distance_p95 = Some(40000.0);

    let hot_scored = BenefitCalculator::score(&hot, &config).unwrap();
    let cold_scored = BenefitCalculator::score(&cold, &config).unwrap();

    // V1 would score them identically (same request_count)
    // V2 should rank hot much higher
    assert!(
        hot_scored.net_benefit > cold_scored.net_benefit,
        "V2 should rank low-reuse-distance object higher: hot={}, cold={}",
        hot_scored.net_benefit,
        cold_scored.net_benefit
    );
}

#[test]
fn v2_ineligible_still_zero() {
    let mut obj = make_object("ineligible-v2", 1024, 1000, false);
    obj.reuse_distance_p50 = Some(10.0);
    let scored = BenefitCalculator::score(&obj, &v2_config()).unwrap();
    assert_eq!(scored.net_benefit, 0.0);
}

#[test]
fn v2_formula_check_single() {
    // Single-object score uses object's own size as mean_size
    let config = v2_config();
    let mut obj = make_object("check-v2", 2048, 500, true);
    obj.update_rate = 0.0;
    obj.mean_reuse_distance = Some(100.0);
    obj.reuse_distance_p50 = Some(100.0);
    obj.reuse_distance_p95 = Some(200.0);

    let scored = BenefitCalculator::score(&obj, &config).unwrap();

    // cache_capacity_objects = 10_000_000 / 2048 (own size) ≈ 4882.8
    let cache_cap = 10_000_000.0 / 2048.0;
    let p_hit = (-100.0_f64 / cache_cap).exp();
    let expected_hits = 500.0 * p_hit;
    let expected_hit_benefit = expected_hits * (50.0 * 0.00005 + 0.003);

    assert!(
        (scored.score_breakdown.expected_hit_benefit - expected_hit_benefit).abs() < 1e-6,
        "V2 hit benefit: got {}, expected {}",
        scored.score_breakdown.expected_hit_benefit,
        expected_hit_benefit
    );
}

#[test]
fn v2_score_all_uses_global_mean_size() {
    // score_all should use global mean object size, not per-object size
    let config = v2_config();

    let mut small = make_object("small", 512, 1000, true);
    small.update_rate = 0.0;
    small.reuse_distance_p50 = Some(100.0);

    let mut large = make_object("large", 8192, 1000, true);
    large.update_rate = 0.0;
    large.reuse_distance_p50 = Some(100.0);

    let objects = vec![small, large];
    let scored = BenefitCalculator::score_all(&objects, &config).unwrap();

    // Both objects have identical reuse distance and request count.
    // With global mean size, cache_capacity_objects is the same for both.
    // The only difference in benefit should come from avg_origin_cost/latency
    // (which are the same in make_object), so p_hit should be identical.
    let mean_size = (512.0 + 8192.0) / 2.0;
    let cache_cap = 10_000_000.0 / mean_size;
    let expected_p_hit = (-100.0_f64 / cache_cap).exp();

    // Verify both objects get the same p_hit (same hit_benefit ratio)
    let benefit_per_request = 50.0 * 0.00005 + 0.003;
    let expected_hit_benefit = 1000.0 * expected_p_hit * benefit_per_request;

    for s in &scored {
        assert!(
            (s.score_breakdown.expected_hit_benefit - expected_hit_benefit).abs() < 1e-6,
            "score_all should use global mean size: key={}, got={}, expected={}",
            s.cache_key,
            s.score_breakdown.expected_hit_benefit,
            expected_hit_benefit
        );
    }
}

#[test]
fn v2_invalidation_model_cost_independent_of_p_hit() {
    // InvalidationOnUpdate freshness cost = update_rate * time_window * invalidation_cost
    // This is a per-update cost, independent of hit probability (by design).
    let config = ScenarioConfig {
        capacity_bytes: 10_000_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.0001,
        freshness_model: FreshnessModel::InvalidationOnUpdate {
            invalidation_cost: 0.001,
        },
        scoring_version: ScoringVersion::V2ReuseDistance,
    };

    let mut obj = make_object("inv-v2", 1024, 1000, true);
    obj.update_rate = 0.001;
    obj.reuse_distance_p50 = Some(100.0);

    let scored = BenefitCalculator::score(&obj, &config).unwrap();

    // Freshness cost should be identical to V1 (invalidation cost is per-update)
    let expected_freshness = 0.001 * 86400.0 * 0.001;
    assert!(
        (scored.score_breakdown.freshness_cost - expected_freshness).abs() < 1e-9,
        "InvalidationOnUpdate cost should be independent of p_hit: got {}, expected {}",
        scored.score_breakdown.freshness_cost,
        expected_freshness
    );

    // But hit benefit should be reduced by p_hit
    let cache_cap = 10_000_000.0 / 1024.0;
    let p_hit = (-100.0_f64 / cache_cap).exp();
    let expected_hit = 1000.0 * p_hit * (50.0 * 0.0001 + 0.003);
    assert!(
        (scored.score_breakdown.expected_hit_benefit - expected_hit).abs() < 1e-6,
        "hit benefit should be scaled by p_hit: got {}, expected {}",
        scored.score_breakdown.expected_hit_benefit,
        expected_hit
    );
}

#[test]
fn v2_aggregate_features_populates_reuse_distance() {
    // Integration test: generate trace → aggregate → check reuse distance fields
    let syn_config = qc_simulate::synthetic::SyntheticConfig {
        num_objects: 100,
        num_requests: 10_000,
        seed: 42,
        ..Default::default()
    };
    let events = qc_simulate::synthetic::generate(&syn_config).unwrap();
    let features = qc_simulate::synthetic::aggregate_features(&events, 86400);

    let has_rd = features
        .iter()
        .filter(|f| f.reuse_distance_p50.is_some())
        .count();

    // Most objects accessed more than once should have reuse distance
    assert!(
        has_rd > 50,
        "expected most objects to have reuse distance, got {has_rd}/{}",
        features.len()
    );
}

#[test]
fn v2_vs_v1_ab_comparison_on_synthetic() {
    // A/B comparison: V1 and V2 should produce different rankings on the same trace
    let syn_config = qc_simulate::synthetic::SyntheticConfig {
        num_objects: 200,
        num_requests: 50_000,
        seed: 123,
        ..Default::default()
    };
    let events = qc_simulate::synthetic::generate(&syn_config).unwrap();
    let features = qc_simulate::synthetic::aggregate_features(&events, 86400);

    let v1_config = ttl_only_config();
    let v2_config = v2_config();

    let v1_scored = BenefitCalculator::score_all(&features, &v1_config).unwrap();
    let v2_scored = BenefitCalculator::score_all(&features, &v2_config).unwrap();

    // V1 and V2 should produce different net_benefit values for objects with reuse distance
    let mut diffs = 0;
    for (s1, s2) in v1_scored.iter().zip(v2_scored.iter()) {
        if (s1.net_benefit - s2.net_benefit).abs() > 1e-9 {
            diffs += 1;
        }
    }

    assert!(
        diffs > 0,
        "V2 should produce different scores from V1 for at least some objects"
    );

    // V2 should produce meaningfully different total benefit (either direction)
    // When net_benefit is positive, V2 total < V1 (hit probability < 1 reduces hits).
    // When net_benefit is negative (freshness cost dominates), V2 may be higher
    // because it also discounts the freshness cost applied to expected hits.
    let v1_total: f64 = v1_scored.iter().map(|s| s.net_benefit).sum();
    let v2_total: f64 = v2_scored.iter().map(|s| s.net_benefit).sum();
    assert!(
        (v1_total - v2_total).abs() > 1.0,
        "V2 total should differ meaningfully from V1: v1={v1_total:.2}, v2={v2_total:.2}"
    );
}

// ── Edge Cases & Backward Compatibility ────────────────────────────

#[test]
fn v2_zero_capacity_returns_zero_not_nan() {
    let config = ScenarioConfig {
        capacity_bytes: 0,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.00005,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::Medium,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
        scoring_version: ScoringVersion::V2ReuseDistance,
    };

    let mut obj = make_object("zero-cap", 1024, 1000, true);
    obj.reuse_distance_p50 = Some(0.0);

    let scored = BenefitCalculator::score(&obj, &config).unwrap();
    assert!(
        scored.net_benefit.is_finite(),
        "zero capacity should not produce NaN: got {}",
        scored.net_benefit
    );
    assert_eq!(scored.net_benefit, 0.0, "zero capacity → zero benefit");
}

#[test]
fn scoring_version_deserialize_backward_compat() {
    // Old config without scoring_version should deserialize with V1 default
    let json = r#"{
        "capacity_bytes": 1000000,
        "time_window_seconds": 86400,
        "latency_value_per_ms": 0.00005,
        "freshness_model": {
            "type": "TtlOnly",
            "stale_penalty": {
                "default_class": "medium",
                "cost_overrides": {}
            }
        }
    }"#;
    let config: ScenarioConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.scoring_version, ScoringVersion::V1Frequency);
}

#[test]
fn scoring_version_deserialize_v2_short_name() {
    // "v2" should work (serde rename)
    let json = r#"{
        "capacity_bytes": 1000000,
        "time_window_seconds": 86400,
        "latency_value_per_ms": 0.00005,
        "freshness_model": {
            "type": "TtlOnly",
            "stale_penalty": {
                "default_class": "medium",
                "cost_overrides": {}
            }
        },
        "scoring_version": "v2"
    }"#;
    let config: ScenarioConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.scoring_version, ScoringVersion::V2ReuseDistance);
}

#[test]
fn scoring_version_deserialize_v2_long_alias() {
    // "v2_reuse_distance" should also work (serde alias)
    let json = r#"{
        "capacity_bytes": 1000000,
        "time_window_seconds": 86400,
        "latency_value_per_ms": 0.00005,
        "freshness_model": {
            "type": "TtlOnly",
            "stale_penalty": {
                "default_class": "medium",
                "cost_overrides": {}
            }
        },
        "scoring_version": "v2_reuse_distance"
    }"#;
    let config: ScenarioConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.scoring_version, ScoringVersion::V2ReuseDistance);
}

#[test]
fn aggregate_features_without_reuse_leaves_none() {
    let syn_config = qc_simulate::synthetic::SyntheticConfig {
        num_objects: 50,
        num_requests: 5_000,
        seed: 99,
        ..Default::default()
    };
    let events = qc_simulate::synthetic::generate(&syn_config).unwrap();
    let features = qc_simulate::synthetic::aggregate_features_with_options(&events, 86400, false);

    let has_rd = features
        .iter()
        .filter(|f| f.reuse_distance_p50.is_some())
        .count();
    assert_eq!(has_rd, 0, "compute_reuse=false should leave all rd as None");
}
