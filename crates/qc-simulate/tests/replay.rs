use chrono::Utc;
use qc_model::trace::RequestTraceEvent;
use qc_simulate::baselines::{GdsfPolicy, LruPolicy, StaticPolicy};
use qc_simulate::comparator::Comparator;
use qc_simulate::engine::{CacheOutcome, CachePolicy, TraceReplayEngine};
use qc_simulate::error::SimulateError;

fn make_event(key: &str, size: u64, cost: Option<f64>) -> RequestTraceEvent {
    RequestTraceEvent {
        schema_version: "1.0".into(),
        timestamp: Utc::now(),
        object_id: key.into(),
        cache_key: key.into(),
        object_size_bytes: size,
        response_bytes: None,
        cache_status: None,
        status_code: Some(200),
        origin_fetch_cost: cost,
        response_latency_ms: Some(50.0),
        region: None,
        content_type: None,
        version_or_etag: None,
        eligible_for_cache: true,
    }
}

fn make_bypass_event(key: &str) -> RequestTraceEvent {
    RequestTraceEvent {
        schema_version: "1.0".into(),
        timestamp: Utc::now(),
        object_id: key.into(),
        cache_key: key.into(),
        object_size_bytes: 100,
        response_bytes: None,
        cache_status: None,
        status_code: Some(200),
        origin_fetch_cost: Some(0.01),
        response_latency_ms: None,
        region: None,
        content_type: None,
        version_or_etag: None,
        eligible_for_cache: false,
    }
}

// ── TraceReplayEngine ───────────────────────────────────────────────

#[test]
fn replay_empty_trace_errors() {
    let mut policy = StaticPolicy::new(std::iter::empty::<String>());
    let result = TraceReplayEngine::replay(&[], &mut policy);
    assert!(matches!(result, Err(SimulateError::EmptyTrace)));
}

#[test]
fn replay_all_hits() {
    let events = vec![
        make_event("a", 100, Some(0.01)),
        make_event("a", 100, Some(0.01)),
        make_event("b", 200, Some(0.02)),
    ];
    let mut policy = StaticPolicy::new(["a".to_string(), "b".to_string()]);
    let metrics = TraceReplayEngine::replay(&events, &mut policy).unwrap();

    assert_eq!(metrics.total_requests, 3);
    assert_eq!(metrics.cache_hits, 3);
    assert_eq!(metrics.cache_misses, 0);
    assert!((metrics.hit_ratio - 1.0).abs() < 1e-9);
    assert!((metrics.estimated_cost_savings - 0.04).abs() < 1e-9);
}

#[test]
fn replay_all_misses() {
    let events = vec![make_event("a", 100, Some(0.01))];
    let mut policy = StaticPolicy::new(std::iter::empty::<String>());
    let metrics = TraceReplayEngine::replay(&events, &mut policy).unwrap();

    assert_eq!(metrics.cache_hits, 0);
    assert_eq!(metrics.cache_misses, 1);
    assert_eq!(metrics.hit_ratio, 0.0);
    assert_eq!(metrics.origin_egress_bytes, 100);
}

#[test]
fn replay_bypass_counted_as_miss() {
    let events = vec![make_bypass_event("x")];
    let mut policy = StaticPolicy::new(["x".to_string()]);
    let metrics = TraceReplayEngine::replay(&events, &mut policy).unwrap();

    assert_eq!(metrics.cache_hits, 0);
    assert_eq!(metrics.cache_misses, 1);
}

// ── LRU Policy ──────────────────────────────────────────────────────

#[test]
fn lru_basic_hit_miss() {
    let mut lru = LruPolicy::new(500);

    // First access is always a miss (cold cache)
    let e1 = make_event("a", 100, None);
    assert_eq!(lru.on_request(&e1), CacheOutcome::Miss);

    // Second access is a hit
    assert_eq!(lru.on_request(&e1), CacheOutcome::Hit);
}

#[test]
fn lru_eviction() {
    // Capacity 200: can hold a(100) + b(100), but not c(100) too
    let mut lru = LruPolicy::new(200);

    let ea = make_event("a", 100, None);
    let eb = make_event("b", 100, None);
    let ec = make_event("c", 100, None);

    lru.on_request(&ea); // miss, cache: [a]
    lru.on_request(&eb); // miss, cache: [a, b]
    lru.on_request(&ec); // miss, evicts a, cache: [b, c]

    assert_eq!(lru.on_request(&ea), CacheOutcome::Miss); // a was evicted
    assert_eq!(lru.on_request(&ec), CacheOutcome::Hit); // c still cached
}

#[test]
fn lru_promote_prevents_eviction() {
    let mut lru = LruPolicy::new(200);

    let ea = make_event("a", 100, None);
    let eb = make_event("b", 100, None);
    let ec = make_event("c", 100, None);

    lru.on_request(&ea); // miss, cache: [a]
    lru.on_request(&eb); // miss, cache: [a, b]
    lru.on_request(&ea); // hit, promote a, cache: [b, a]
    lru.on_request(&ec); // miss, evicts b (LRU), cache: [a, c]

    assert_eq!(lru.on_request(&ea), CacheOutcome::Hit); // a was promoted, still cached
    assert_eq!(lru.on_request(&eb), CacheOutcome::Miss); // b was evicted
}

#[test]
fn lru_oversized_object_skipped() {
    let mut lru = LruPolicy::new(50);
    let big = make_event("big", 100, None);
    assert_eq!(lru.on_request(&big), CacheOutcome::Miss);
    assert_eq!(lru.on_request(&big), CacheOutcome::Miss); // still not cached
}

#[test]
fn lru_bypass_not_cached() {
    let mut lru = LruPolicy::new(1000);
    let bypass = make_bypass_event("x");
    assert_eq!(lru.on_request(&bypass), CacheOutcome::Bypass);
    assert_eq!(lru.on_request(&bypass), CacheOutcome::Bypass);
}

// ── GDSF Policy ─────────────────────────────────────────────────────

#[test]
fn gdsf_basic_hit_miss() {
    let mut gdsf = GdsfPolicy::new(500);
    let e = make_event("a", 100, Some(0.01));
    assert_eq!(gdsf.on_request(&e), CacheOutcome::Miss);
    assert_eq!(gdsf.on_request(&e), CacheOutcome::Hit);
}

#[test]
fn gdsf_evicts_low_priority() {
    // Two items of equal size but different cost
    // Cheap item should be evicted first
    let mut gdsf = GdsfPolicy::new(200);

    let cheap = make_event("cheap", 100, Some(0.001));
    let expensive = make_event("expensive", 100, Some(1.0));
    let newcomer = make_event("new", 100, Some(0.5));

    gdsf.on_request(&cheap); // miss
    gdsf.on_request(&expensive); // miss, cache full
    gdsf.on_request(&newcomer); // miss, evicts cheap (lower priority)

    assert_eq!(gdsf.on_request(&cheap), CacheOutcome::Miss); // cheap was evicted
    assert_eq!(gdsf.on_request(&expensive), CacheOutcome::Hit); // expensive still cached
}

#[test]
fn gdsf_frequency_boosts_priority() {
    let mut gdsf = GdsfPolicy::new(200);

    let frequent = make_event("freq", 100, Some(0.01));
    let rare = make_event("rare", 100, Some(0.01));
    let newcomer = make_event("new", 100, Some(0.01));

    gdsf.on_request(&frequent); // miss
    gdsf.on_request(&rare); // miss
                            // Boost frequent's priority
    gdsf.on_request(&frequent); // hit (freq=2)
    gdsf.on_request(&frequent); // hit (freq=3)

    gdsf.on_request(&newcomer); // miss, should evict rare (freq=1) over frequent (freq=3)

    assert_eq!(gdsf.on_request(&frequent), CacheOutcome::Hit);
    assert_eq!(gdsf.on_request(&rare), CacheOutcome::Miss); // rare was evicted
}

#[test]
fn gdsf_bypass_not_cached() {
    let mut gdsf = GdsfPolicy::new(1000);
    let bypass = make_bypass_event("x");
    assert_eq!(gdsf.on_request(&bypass), CacheOutcome::Bypass);
}

// ── Comparator ──────────────────────────────────────────────────────

#[test]
fn comparator_runs_all_policies() {
    // Simple trace: repeated access to a few objects
    let events: Vec<_> = (0..100)
        .map(|i| {
            let key = format!("obj-{}", i % 10);
            make_event(&key, 100, Some(0.01))
        })
        .collect();

    let mut static_policy = StaticPolicy::new(
        (0..5).map(|i| format!("obj-{i}")), // cache first 5
    );
    let mut lru = LruPolicy::new(1200); // fits all 10 objects (100 bytes each)
    let mut gdsf = GdsfPolicy::new(1200);

    let report = Comparator::compare(
        &events,
        &mut [
            &mut static_policy as &mut dyn CachePolicy,
            &mut lru,
            &mut gdsf,
        ],
    )
    .unwrap();

    assert_eq!(report.results.len(), 3);
    assert_eq!(report.results[0].name, "EconomicGreedy");
    assert_eq!(report.results[1].name, "LRU");
    assert_eq!(report.results[2].name, "GDSF");

    // All policies should have some hits
    for r in &report.results {
        assert!(r.metrics.hit_ratio > 0.0, "{} should have hits", r.name);
    }

    // Static caches exactly keys obj-0..obj-4 → 50% hit ratio
    assert!(
        (report.results[0].metrics.hit_ratio - 0.5).abs() < 1e-9,
        "static should have 50% hits"
    );
}

#[test]
fn comparator_best_by_methods() {
    let events: Vec<_> = (0..50)
        .map(|i| make_event(&format!("obj-{}", i % 5), 100, Some(0.01)))
        .collect();

    let mut policy_a = StaticPolicy::new(
        (0..5).map(|i| format!("obj-{i}")), // 100% hit
    );
    let mut policy_b = StaticPolicy::new(std::iter::empty::<String>()); // 0% hit

    let report = Comparator::compare(
        &events,
        &mut [
            &mut policy_a as &mut dyn CachePolicy,
            &mut policy_b as &mut dyn CachePolicy,
        ],
    )
    .unwrap();

    let best = report.best_by_hit_ratio().unwrap();
    assert_eq!(best.name, "EconomicGreedy"); // first one has 100%

    let best_cost = report.best_by_cost_savings().unwrap();
    assert!(best_cost.metrics.estimated_cost_savings > 0.0);
}

// ── Full pipeline: 4 policies on same trace ─────────────────────────

#[test]
fn four_policy_comparison() {
    // Build a trace with skewed popularity (Zipf-like)
    let mut events = Vec::new();
    for _ in 0..1000 {
        // obj-0 is most popular, obj-9 is least
        for i in 0..10 {
            let repeats = 100 / (i + 1); // obj-0: 100, obj-1: 50, ..., obj-9: 10
            for _ in 0..repeats {
                events.push(make_event(
                    &format!("obj-{i}"),
                    (i + 1) as u64 * 100, // varying sizes
                    Some(0.01),
                ));
            }
        }
    }

    let capacity = 2000; // limited capacity

    // Policy 1: Static (top-3 by hand)
    let mut static_pol =
        StaticPolicy::new(["obj-0", "obj-1", "obj-2"].iter().map(|s| s.to_string()));

    // Policy 2: LRU
    let mut lru = LruPolicy::new(capacity);

    // Policy 3: GDSF
    let mut gdsf = GdsfPolicy::new(capacity);

    // Policy 4: Empty (no caching baseline)
    let mut nocache = StaticPolicy::new(std::iter::empty::<String>());

    let report = Comparator::compare(
        &events,
        &mut [
            &mut static_pol as &mut dyn CachePolicy,
            &mut lru,
            &mut gdsf,
            &mut nocache,
        ],
    )
    .unwrap();

    assert_eq!(report.results.len(), 4);

    // No-cache baseline should have 0 hits
    assert_eq!(report.results[3].metrics.cache_hits, 0);

    // All other policies should outperform no-cache
    for r in &report.results[..3] {
        assert!(
            r.metrics.hit_ratio > 0.0,
            "{} should outperform no-cache",
            r.name
        );
    }

    // Print results for visibility
    for r in &report.results {
        println!(
            "{:8} | hit_ratio: {:.2}% | cost_savings: ${:.4} | byte_hit_ratio: {:.2}%",
            r.name,
            r.metrics.hit_ratio * 100.0,
            r.metrics.estimated_cost_savings,
            r.metrics.byte_hit_ratio * 100.0,
        );
    }
}
