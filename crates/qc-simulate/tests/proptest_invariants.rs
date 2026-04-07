use proptest::prelude::*;

use qc_model::trace::RequestTraceEvent;
use qc_simulate::baselines::LruPolicy;
use qc_simulate::engine::{CachePolicy, TraceReplayEngine};

fn arb_trace_event() -> impl Strategy<Value = RequestTraceEvent> {
    (
        1u64..100_000,                   // size_bytes
        prop::bool::ANY,                 // eligible
        prop::option::of(1.0f64..100.0), // origin_cost
    )
        .prop_map(|(size, eligible, cost)| RequestTraceEvent {
            schema_version: "1.0".into(),
            timestamp: chrono::DateTime::from_timestamp(0, 0).unwrap(),
            object_id: "obj".into(),
            cache_key: format!("/obj-{size}"),
            object_size_bytes: size,
            response_bytes: Some(size),
            cache_status: None,
            status_code: Some(200),
            origin_fetch_cost: cost,
            response_latency_ms: Some(10.0),
            region: None,
            content_type: None,
            version_or_etag: None,
            eligible_for_cache: eligible,
        })
}

proptest! {
    /// R1: hit + miss = total requests.
    #[test]
    fn hit_plus_miss_equals_total(
        events in prop::collection::vec(arb_trace_event(), 1..100),
        cap in 1u64..1_000_000,
    ) {
        let mut policy = LruPolicy::new(cap);
        let metrics = TraceReplayEngine::replay(&events, &mut policy).unwrap();
        prop_assert_eq!(
            metrics.cache_hits + metrics.cache_misses,
            metrics.total_requests,
            "hits {} + misses {} != total {}",
            metrics.cache_hits, metrics.cache_misses, metrics.total_requests
        );
    }

    /// R2: byte_hit_ratio is in [0, 1].
    #[test]
    fn byte_hit_ratio_in_range(
        events in prop::collection::vec(arb_trace_event(), 1..100),
        cap in 1u64..1_000_000,
    ) {
        let mut policy = LruPolicy::new(cap);
        let metrics = TraceReplayEngine::replay(&events, &mut policy).unwrap();
        prop_assert!(metrics.byte_hit_ratio >= 0.0 && metrics.byte_hit_ratio <= 1.0,
            "byte_hit_ratio {} out of [0,1]", metrics.byte_hit_ratio);
    }

    /// R5: LRU never exceeds capacity constraint.
    #[test]
    fn lru_never_exceeds_capacity(
        events in prop::collection::vec(arb_trace_event(), 1..200),
        cap in 1u64..500_000,
    ) {
        let mut policy = LruPolicy::new(cap);
        for event in &events {
            policy.on_request(event);
            prop_assert!(policy.used_bytes() <= cap,
                "LRU used {} > capacity {} after processing {}",
                policy.used_bytes(), cap, event.cache_key);
        }
    }
}
