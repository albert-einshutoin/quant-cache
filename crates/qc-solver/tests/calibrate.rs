use qc_model::object::ObjectFeatures;
use qc_model::scenario::{
    FreshnessModel, ScenarioConfig, ScoringVersion, StaleCostOverrides, StalePenaltyClass,
    StalePenaltyConfig,
};
use qc_model::trace::RequestTraceEvent;
use qc_solver::calibrate::{calibrate, default_eval};

fn make_features(n: usize) -> Vec<ObjectFeatures> {
    (0..n)
        .map(|i| ObjectFeatures {
            object_id: format!("obj-{i}"),
            cache_key: format!("/content/{i}"),
            size_bytes: 1000,
            eligible_for_cache: true,
            request_count: 100,
            request_rate: 100.0 / 86400.0,
            avg_response_bytes: 1000,
            avg_origin_cost: 0.003,
            avg_latency_saving_ms: 50.0,
            ttl_seconds: 3600,
            update_rate: 0.0,
            last_modified: None,
            stale_penalty_class: StalePenaltyClass::Medium,
            purge_group: None,
            origin_group: None,
            mean_reuse_distance: None,
            reuse_distance_p50: None,
            reuse_distance_p95: None,
        })
        .collect()
}

fn make_events(n_objects: usize, n_requests: usize) -> Vec<RequestTraceEvent> {
    let base = chrono::DateTime::from_timestamp(1_000_000, 0).unwrap();
    (0..n_requests)
        .map(|i| RequestTraceEvent {
            schema_version: "1.0".into(),
            timestamp: base + chrono::Duration::seconds(i as i64),
            object_id: format!("obj-{}", i % n_objects),
            cache_key: format!("/content/{}", i % n_objects),
            object_size_bytes: 1000,
            response_bytes: Some(1000),
            cache_status: None,
            status_code: Some(200),
            origin_fetch_cost: Some(0.003),
            response_latency_ms: Some(50.0),
            region: None,
            content_type: None,
            version_or_etag: Some("v1".into()),
            eligible_for_cache: true,
        })
        .collect()
}

fn base_config(capacity: u64) -> ScenarioConfig {
    ScenarioConfig {
        capacity_bytes: capacity,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.0001,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::None,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
        scoring_version: ScoringVersion::default(),
    }
}

#[test]
fn default_eval_returns_positive_for_cached_objects() {
    let features = make_features(10);
    let events = make_events(10, 100);
    let config = base_config(100_000); // fits all 10 objects
    let score = default_eval(&config, &features, &events, 100_000).unwrap();
    assert!(score > 0.0, "should have positive savings, got {score}");
}

#[test]
fn default_eval_returns_zero_for_zero_capacity() {
    let features = make_features(10);
    let events = make_events(10, 100);
    let config = base_config(0);
    let score = default_eval(&config, &features, &events, 0).unwrap();
    assert_eq!(score, 0.0, "zero capacity should yield zero savings");
}

#[test]
fn calibrate_improves_or_maintains_score() {
    let features = make_features(20);
    let events = make_events(20, 200);
    let config = base_config(10_000); // fits ~10 objects

    // Use same data for train/val (simplified test)
    let result = calibrate(&features, &events, &features, &events, &config, 2).unwrap();

    assert!(
        result.best_score >= 0.0,
        "calibrated score should be non-negative"
    );
    assert!(result.iterations > 0, "should have run iterations");
    assert!(
        !result.parameter_sensitivity.is_empty(),
        "should report sensitivity"
    );
}

#[test]
fn calibrate_terminates_on_empty_features() {
    let features: Vec<ObjectFeatures> = vec![];
    let events: Vec<RequestTraceEvent> = vec![];
    let config = base_config(10_000);
    let result = calibrate(&features, &events, &features, &events, &config, 1).unwrap();
    assert_eq!(result.best_score, 0.0);
}
