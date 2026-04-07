/// Parity tests: verify compact (u32) and string-based replay produce identical metrics.
use qc_model::compact_trace::CompactTraceEvent;
use qc_model::scenario::StalePenaltyClass;
use qc_simulate::baselines::{BeladyPolicy, LruPolicy, SievePolicy, StaticPolicy};
use qc_simulate::compact_baselines::{
    CompactBeladyPolicy, CompactLruPolicy, CompactSievePolicy, CompactStaticPolicy,
};
use qc_simulate::engine::{CompactReplayEconConfig, ReplayEconConfig, TraceReplayEngine};
use qc_simulate::synthetic::{self, SyntheticConfig};

fn test_trace() -> Vec<qc_model::trace::RequestTraceEvent> {
    let config = SyntheticConfig {
        num_objects: 200,
        num_requests: 10_000,
        zipf_alpha: 0.8,
        seed: 42,
        ..SyntheticConfig::default()
    };
    synthetic::generate(&config).unwrap()
}

fn assert_metrics_equal(
    name: &str,
    string_metrics: &qc_model::metrics::MetricsSummary,
    compact_metrics: &qc_model::metrics::MetricsSummary,
) {
    assert_eq!(
        string_metrics.total_requests, compact_metrics.total_requests,
        "{name}: total_requests mismatch"
    );
    assert_eq!(
        string_metrics.cache_hits, compact_metrics.cache_hits,
        "{name}: cache_hits mismatch"
    );
    assert_eq!(
        string_metrics.cache_misses, compact_metrics.cache_misses,
        "{name}: cache_misses mismatch"
    );
    assert!(
        (string_metrics.hit_ratio - compact_metrics.hit_ratio).abs() < 1e-12,
        "{name}: hit_ratio mismatch: {} vs {}",
        string_metrics.hit_ratio,
        compact_metrics.hit_ratio
    );
    assert!(
        (string_metrics.estimated_cost_savings - compact_metrics.estimated_cost_savings).abs()
            < 1e-6,
        "{name}: cost_savings mismatch"
    );
}

#[test]
fn lru_parity() {
    let events = test_trace();
    let capacity = 500_000u64;

    // String path
    let mut lru_string = LruPolicy::new(capacity);
    let s_metrics = TraceReplayEngine::replay(&events, &mut lru_string).unwrap();

    // Compact path
    let (compact_events, _interner) = CompactTraceEvent::intern_batch(&events);
    let mut lru_compact = CompactLruPolicy::new(capacity);
    let c_metrics = TraceReplayEngine::replay_compact_with_econ(
        &compact_events,
        &mut lru_compact,
        &CompactReplayEconConfig::from_econ_config(
            &ReplayEconConfig::default(),
            &mut qc_model::intern::StringInterner::new(),
        ),
    )
    .unwrap();

    assert_metrics_equal("LRU", &s_metrics, &c_metrics);
}

#[test]
fn sieve_parity() {
    let events = test_trace();
    let capacity = 500_000u64;

    let mut sieve_string = SievePolicy::new(capacity);
    let s_metrics = TraceReplayEngine::replay(&events, &mut sieve_string).unwrap();

    let (compact_events, _interner) = CompactTraceEvent::intern_batch(&events);
    let mut sieve_compact = CompactSievePolicy::new(capacity);
    let c_metrics = TraceReplayEngine::replay_compact_with_econ(
        &compact_events,
        &mut sieve_compact,
        &CompactReplayEconConfig::from_econ_config(
            &ReplayEconConfig::default(),
            &mut qc_model::intern::StringInterner::new(),
        ),
    )
    .unwrap();

    assert_metrics_equal("SIEVE", &s_metrics, &c_metrics);
}

#[test]
fn static_policy_parity() {
    let events = test_trace();

    // Cache top 50 objects by frequency
    let mut freq: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for e in &events {
        *freq.entry(&e.cache_key).or_insert(0) += 1;
    }
    let mut by_freq: Vec<(&str, u64)> = freq.into_iter().collect();
    by_freq.sort_by(|a, b| b.1.cmp(&a.1));
    let top_keys: Vec<String> = by_freq
        .iter()
        .take(50)
        .map(|(k, _)| k.to_string())
        .collect();

    // String path
    let mut static_string = StaticPolicy::new(top_keys.iter().cloned());
    let s_metrics = TraceReplayEngine::replay(&events, &mut static_string).unwrap();

    // Compact path
    let (compact_events, mut interner) = CompactTraceEvent::intern_batch(&events);
    let top_ids: Vec<u32> = top_keys.iter().map(|k| interner.intern(k)).collect();
    let mut static_compact = CompactStaticPolicy::new(top_ids);
    let c_metrics = TraceReplayEngine::replay_compact_with_econ(
        &compact_events,
        &mut static_compact,
        &CompactReplayEconConfig::from_econ_config(&ReplayEconConfig::default(), &mut interner),
    )
    .unwrap();

    assert_metrics_equal("StaticPolicy", &s_metrics, &c_metrics);
}

#[test]
fn belady_parity() {
    let events = test_trace();
    let capacity = 500_000u64;

    let mut belady_string = BeladyPolicy::new(&events, capacity);
    let s_metrics = TraceReplayEngine::replay(&events, &mut belady_string).unwrap();

    let (compact_events, _interner) = CompactTraceEvent::intern_batch(&events);
    let mut belady_compact = CompactBeladyPolicy::new(&compact_events, capacity);
    let c_metrics = TraceReplayEngine::replay_compact_with_econ(
        &compact_events,
        &mut belady_compact,
        &CompactReplayEconConfig::from_econ_config(
            &ReplayEconConfig::default(),
            &mut qc_model::intern::StringInterner::new(),
        ),
    )
    .unwrap();

    assert_metrics_equal("Belady", &s_metrics, &c_metrics);
}

#[test]
fn lru_parity_with_econ() {
    let events = test_trace();
    let capacity = 500_000u64;
    let features = synthetic::aggregate_features(&events, 86400);
    let econ = ReplayEconConfig::from_features(&features, 0.0001, StalePenaltyClass::Low);

    let mut lru_string = LruPolicy::new(capacity);
    let s_metrics = TraceReplayEngine::replay_with_econ(&events, &mut lru_string, &econ).unwrap();

    let (compact_events, mut interner) = CompactTraceEvent::intern_batch(&events);
    let compact_econ = CompactReplayEconConfig::from_econ_config(&econ, &mut interner);
    let mut lru_compact = CompactLruPolicy::new(capacity);
    let c_metrics = TraceReplayEngine::replay_compact_with_econ(
        &compact_events,
        &mut lru_compact,
        &compact_econ,
    )
    .unwrap();

    assert_metrics_equal("LRU+econ", &s_metrics, &c_metrics);
}
