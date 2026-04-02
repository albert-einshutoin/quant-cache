use qc_model::object::{ObjectFeatures, ScoreBreakdown, ScoredObject};
use qc_model::policy_ir::*;
use qc_model::trace::RequestTraceEvent;
use qc_simulate::baselines::SievePolicy;
use qc_simulate::engine::{CacheOutcome, CachePolicy, TraceReplayEngine};
use qc_simulate::ir_policy::{IrEvalContext, IrPolicy};
use qc_simulate::synthetic::{self, SyntheticConfig};

fn make_scored(key: &str, size: u64, benefit: f64) -> ScoredObject {
    ScoredObject {
        object_id: key.into(),
        cache_key: key.into(),
        size_bytes: size,
        net_benefit: benefit,
        score_breakdown: ScoreBreakdown {
            expected_hit_benefit: benefit.max(0.0),
            freshness_cost: 0.0,
            net_benefit: benefit,
            capacity_shadow_cost: None,
        },
    }
}

fn generate_small_trace() -> (
    Vec<RequestTraceEvent>,
    Vec<ObjectFeatures>,
    Vec<ScoredObject>,
) {
    let config = SyntheticConfig {
        num_objects: 50,
        num_requests: 2000,
        seed: 99,
        ..SyntheticConfig::default()
    };
    let events = synthetic::generate(&config).unwrap();
    let features = synthetic::aggregate_features(&events, config.time_window_seconds);

    let scored: Vec<ScoredObject> = features
        .iter()
        .map(|f| make_scored(&f.cache_key, f.size_bytes, f.request_count as f64 * 0.01))
        .collect();

    (events, features, scored)
}

#[test]
fn ir_sieve_always_matches_pure_sieve() {
    let (events, features, scored) = generate_small_trace();
    let capacity = 100_000u64;

    // Pure SIEVE
    let mut sieve = SievePolicy::new(capacity);
    let sieve_metrics = TraceReplayEngine::replay(&events, &mut sieve).unwrap();

    // IR(SIEVE, Always)
    let ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: capacity,
        admission_rule: AdmissionRule::Always,
        bypass_rule: BypassRule::None,
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
    let mut ir_policy = IrPolicy::new(ir, ctx);
    let ir_metrics = TraceReplayEngine::replay(&events, &mut ir_policy).unwrap();

    assert_eq!(sieve_metrics.cache_hits, ir_metrics.cache_hits);
    assert_eq!(sieve_metrics.cache_misses, ir_metrics.cache_misses);
    assert!((sieve_metrics.hit_ratio - ir_metrics.hit_ratio).abs() < 1e-9);
}

#[test]
fn admission_threshold_filters_objects() {
    let (events, features, scored) = generate_small_trace();

    // Very high threshold — almost nothing admitted
    let ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: 1_000_000,
        admission_rule: AdmissionRule::ScoreThreshold { threshold: 999.0 },
        bypass_rule: BypassRule::None,
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
    let mut policy = IrPolicy::new(ir, ctx);
    let metrics = TraceReplayEngine::replay(&events, &mut policy).unwrap();

    assert!(
        metrics.hit_ratio < 0.05,
        "with threshold=999, almost nothing should be cached, got {:.2}%",
        metrics.hit_ratio * 100.0
    );
}

#[test]
fn bypass_size_limit_works() {
    let (events, features, scored) = generate_small_trace();

    // Bypass objects > 100 bytes (most objects are larger)
    let ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: 1_000_000,
        admission_rule: AdmissionRule::Always,
        bypass_rule: BypassRule::SizeLimit { max_bytes: 100 },
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
    let mut policy = IrPolicy::new(ir, ctx);
    let metrics = TraceReplayEngine::replay(&events, &mut policy).unwrap();

    // Most synthetic objects are > 100 bytes, so nearly all should be bypassed
    assert!(
        metrics.hit_ratio < 0.1,
        "with 100-byte size limit, most objects should be bypassed"
    );
}

#[test]
fn prewarm_causes_first_hit() {
    let (events, features, scored) = generate_small_trace();
    let first_key = events
        .iter()
        .find(|e| e.eligible_for_cache)
        .unwrap()
        .cache_key
        .clone();

    let ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: 1_000_000,
        admission_rule: AdmissionRule::Always,
        bypass_rule: BypassRule::None,
        prewarm_set: vec![first_key.clone()],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
    let mut policy = IrPolicy::new(ir, ctx);

    let trace_start = events.first().unwrap().timestamp;
    policy.prewarm(&features, trace_start);

    // First request to prewarm key should be a hit
    let first_event = events.iter().find(|e| e.cache_key == first_key).unwrap();
    let outcome = policy.on_request(first_event);
    assert_eq!(
        outcome,
        CacheOutcome::Hit,
        "prewarm object should hit on first access"
    );
}

#[test]
fn composite_bypass_any() {
    let (events, features, scored) = generate_small_trace();

    let ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: 1_000_000,
        admission_rule: AdmissionRule::Always,
        bypass_rule: BypassRule::Any {
            rules: vec![
                BypassRule::SizeLimit { max_bytes: 100 },
                BypassRule::FreshnessRisk { threshold: 0.01 },
            ],
        },
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
    let mut policy = IrPolicy::new(ir, ctx);
    let metrics = TraceReplayEngine::replay(&events, &mut policy).unwrap();

    // Composite should bypass more than either rule alone
    assert!(
        metrics.hit_ratio < 0.1,
        "composite bypass should filter aggressively"
    );
}

#[test]
fn cache_key_rules_normalize_keys() {
    let (events, features, scored) = generate_small_trace();

    // Rule: strip everything after ? (query params)
    let ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: 1_000_000,
        admission_rule: AdmissionRule::Always,
        bypass_rule: BypassRule::None,
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![qc_model::policy_ir::CacheKeyRule {
            pattern: r"\?.*$".to_string(),
            replacement: "".to_string(),
        }],
    };

    let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
    let mut policy = IrPolicy::new(ir, ctx);

    // Verify: two events with same path but different query params
    // should map to the same cache entry (second one is a hit)
    let mut e1 = events[0].clone();
    e1.cache_key = "/page?utm_source=google".to_string();
    e1.eligible_for_cache = true;

    let mut e2 = e1.clone();
    e2.cache_key = "/page?utm_source=twitter".to_string();

    let r1 = policy.on_request(&e1);
    let r2 = policy.on_request(&e2);

    assert_eq!(r1, CacheOutcome::Miss, "first request is a miss");
    assert_eq!(
        r2,
        CacheOutcome::Hit,
        "second request with different query params should hit (normalized to same key)"
    );
}
