use chrono::Utc;
use qc_model::metrics::MetricsSummary;
use qc_model::object::{ObjectFeatures, ScoreBreakdown, ScoredObject};
use qc_model::policy::PolicyDecision;
use qc_model::scenario::{
    CapacityConstraint, FreshnessModel, ScenarioConfig, StaleCostOverrides, StalePenaltyClass,
    StalePenaltyConfig,
};
use qc_model::trace::{CacheStatus, RequestTraceEvent};

/// Helper: serialize to JSON and deserialize back, assert equality via re-serialization.
fn roundtrip<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) {
    let json = serde_json::to_string_pretty(value).expect("serialize");
    let back: T = serde_json::from_str(&json).expect("deserialize");
    let json2 = serde_json::to_string_pretty(&back).expect("re-serialize");
    assert_eq!(json, json2, "roundtrip mismatch");
}

// ── RequestTraceEvent ───────────────────────────────────────────────

#[test]
fn trace_event_full_roundtrip() {
    let event = RequestTraceEvent {
        schema_version: RequestTraceEvent::SCHEMA_VERSION.to_string(),
        timestamp: Utc::now(),
        object_id: "obj-001".into(),
        cache_key: "/img/hero.png".into(),
        object_size_bytes: 102_400,
        response_bytes: Some(102_400),
        cache_status: Some(CacheStatus::Hit),
        status_code: Some(200),
        origin_fetch_cost: Some(0.005),
        response_latency_ms: Some(12.3),
        region: Some("ap-northeast-1".into()),
        content_type: Some("image/png".into()),
        version_or_etag: Some("abc123".into()),
        eligible_for_cache: true,
    };
    roundtrip(&event);
}

#[test]
fn trace_event_minimal_roundtrip() {
    let event = RequestTraceEvent {
        schema_version: RequestTraceEvent::SCHEMA_VERSION.to_string(),
        timestamp: Utc::now(),
        object_id: "obj-002".into(),
        cache_key: "/api/data".into(),
        object_size_bytes: 256,
        response_bytes: None,
        cache_status: None,
        status_code: None,
        origin_fetch_cost: None,
        response_latency_ms: None,
        region: None,
        content_type: None,
        version_or_etag: None,
        eligible_for_cache: false,
    };
    roundtrip(&event);
}

#[test]
fn cache_status_all_variants() {
    for status in [
        CacheStatus::Hit,
        CacheStatus::Miss,
        CacheStatus::Expired,
        CacheStatus::Bypass,
    ] {
        roundtrip(&status);
    }
}

// ── ObjectFeatures ──────────────────────────────────────────────────

#[test]
fn object_features_roundtrip() {
    let obj = ObjectFeatures {
        object_id: "obj-100".into(),
        cache_key: "/products/100".into(),
        size_bytes: 8192,
        eligible_for_cache: true,
        request_count: 5000,
        request_rate: 0.058,
        avg_response_bytes: 8192,
        avg_origin_cost: 0.003,
        avg_latency_saving_ms: 50.0,
        ttl_seconds: 3600,
        update_rate: 0.001,
        last_modified: Some(Utc::now()),
        stale_penalty_class: StalePenaltyClass::Medium,
        purge_group: Some("products".into()),
        origin_group: Some("api-origin".into()),
    };
    roundtrip(&obj);
}

// ── ScoredObject + ScoreBreakdown ───────────────────────────────────

#[test]
fn scored_object_roundtrip() {
    let scored = ScoredObject {
        object_id: "obj-100".into(),
        cache_key: "/products/100".into(),
        size_bytes: 8192,
        net_benefit: 12.5,
        score_breakdown: ScoreBreakdown {
            expected_hit_benefit: 15.0,
            freshness_cost: 2.5,
            net_benefit: 12.5,
            capacity_shadow_cost: Some(0.0015),
        },
    };
    roundtrip(&scored);
}

#[test]
fn score_breakdown_no_shadow_cost() {
    let bd = ScoreBreakdown {
        expected_hit_benefit: 10.0,
        freshness_cost: 1.0,
        net_benefit: 9.0,
        capacity_shadow_cost: None,
    };
    roundtrip(&bd);
}

// ── PolicyDecision ──────────────────────────────────────────────────

#[test]
fn policy_decision_roundtrip() {
    let dec = PolicyDecision {
        cache_key: "/img/logo.svg".into(),
        cache: true,
        score: 42.0,
        score_breakdown: ScoreBreakdown {
            expected_hit_benefit: 50.0,
            freshness_cost: 8.0,
            net_benefit: 42.0,
            capacity_shadow_cost: None,
        },
    };
    roundtrip(&dec);
}

// ── ScenarioConfig ──────────────────────────────────────────────────

#[test]
fn scenario_ttl_only_roundtrip() {
    let config = ScenarioConfig {
        capacity_bytes: 10_000_000_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.00005,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::High,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
    };
    roundtrip(&config);
}

#[test]
fn scenario_invalidation_roundtrip() {
    let config = ScenarioConfig {
        capacity_bytes: 5_000_000_000,
        time_window_seconds: 3600,
        latency_value_per_ms: 0.0001,
        freshness_model: FreshnessModel::InvalidationOnUpdate {
            invalidation_cost: 0.001,
        },
    };
    roundtrip(&config);
}

// ── StalePenaltyClass ───────────────────────────────────────────────

#[test]
fn stale_penalty_class_all_variants() {
    for class in [
        StalePenaltyClass::None,
        StalePenaltyClass::Low,
        StalePenaltyClass::Medium,
        StalePenaltyClass::High,
        StalePenaltyClass::VeryHigh,
    ] {
        roundtrip(&class);
    }
}

#[test]
fn stale_penalty_class_to_cost() {
    assert_eq!(StalePenaltyClass::None.to_cost(), 0.0);
    assert_eq!(StalePenaltyClass::Low.to_cost(), 0.001);
    assert_eq!(StalePenaltyClass::Medium.to_cost(), 0.01);
    assert_eq!(StalePenaltyClass::High.to_cost(), 0.1);
    assert_eq!(StalePenaltyClass::VeryHigh.to_cost(), 1.0);
}

// ── CapacityConstraint ──────────────────────────────────────────────

#[test]
fn capacity_constraint_roundtrip() {
    let c = CapacityConstraint {
        capacity_bytes: 1_073_741_824,
    };
    roundtrip(&c);
}

// ── MetricsSummary ──────────────────────────────────────────────────

#[test]
fn metrics_summary_default_roundtrip() {
    let m = MetricsSummary::default();
    roundtrip(&m);
}

#[test]
fn metrics_summary_populated_roundtrip() {
    let m = MetricsSummary {
        total_requests: 1_000_000,
        cache_hits: 850_000,
        cache_misses: 150_000,
        hit_ratio: 0.85,
        total_bytes_served: 50_000_000_000,
        bytes_from_cache: 42_500_000_000,
        byte_hit_ratio: 0.85,
        origin_egress_bytes: 7_500_000_000,
        estimated_cost_savings: 1250.50,
        policy_objective_value: 9800.0,
        stale_serve_count: 500,
        stale_serve_rate: 0.0005,
        policy_churn: 0.02,
        solve_time_ms: 150,
        capacity_utilization: 0.92,
        optimality_gap: Some(0.03),
    };
    roundtrip(&m);
}

// ── Preset ──────────────────────────────────────────────────────────

#[test]
fn preset_generates_valid_configs() {
    use qc_model::preset::Preset;

    for preset in [Preset::Ecommerce, Preset::Media, Preset::Api] {
        let config = preset.to_config(10_000_000_000);
        // Config must roundtrip through JSON
        roundtrip(&config);
        assert_eq!(config.capacity_bytes, 10_000_000_000);
        assert!(config.time_window_seconds > 0);
        assert!(config.latency_value_per_ms > 0.0);
    }
}
