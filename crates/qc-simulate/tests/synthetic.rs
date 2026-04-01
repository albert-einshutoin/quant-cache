use qc_simulate::synthetic::{aggregate_features, generate, SyntheticConfig};

#[test]
fn default_config_generates_correct_count() {
    let config = SyntheticConfig {
        num_requests: 10_000, // reduced for test speed
        num_objects: 100,
        ..SyntheticConfig::default()
    };
    let events = generate(&config).unwrap();
    assert_eq!(events.len(), 10_000);
}

#[test]
fn events_are_sorted_by_timestamp() {
    let config = SyntheticConfig {
        num_requests: 1_000,
        num_objects: 50,
        ..SyntheticConfig::default()
    };
    let events = generate(&config).unwrap();
    for w in events.windows(2) {
        assert!(
            w[0].timestamp <= w[1].timestamp,
            "events must be sorted by timestamp"
        );
    }
}

#[test]
fn deterministic_with_same_seed() {
    let config = SyntheticConfig {
        num_requests: 500,
        num_objects: 20,
        seed: 123,
        ..SyntheticConfig::default()
    };
    let events1 = generate(&config).unwrap();
    let events2 = generate(&config).unwrap();

    assert_eq!(events1.len(), events2.len());
    for (e1, e2) in events1.iter().zip(events2.iter()) {
        assert_eq!(e1.cache_key, e2.cache_key);
        assert_eq!(e1.object_size_bytes, e2.object_size_bytes);
        assert_eq!(e1.timestamp, e2.timestamp);
    }
}

#[test]
fn different_seeds_produce_different_traces() {
    let base = SyntheticConfig {
        num_requests: 500,
        num_objects: 20,
        ..SyntheticConfig::default()
    };
    let events1 = generate(&SyntheticConfig {
        seed: 1,
        ..base.clone()
    })
    .unwrap();
    let events2 = generate(&SyntheticConfig {
        seed: 2,
        ..base.clone()
    })
    .unwrap();

    // At least some events should differ
    let diff_count = events1
        .iter()
        .zip(events2.iter())
        .filter(|(a, b)| a.cache_key != b.cache_key)
        .count();
    assert!(
        diff_count > 0,
        "different seeds should produce different traces"
    );
}

#[test]
fn zipf_skew_produces_popularity_concentration() {
    let config = SyntheticConfig {
        num_requests: 10_000,
        num_objects: 100,
        zipf_alpha: 1.2, // strong skew
        ..SyntheticConfig::default()
    };
    let events = generate(&config).unwrap();

    // Count requests per object
    let mut counts = std::collections::HashMap::new();
    for e in &events {
        *counts.entry(e.cache_key.clone()).or_insert(0u64) += 1;
    }

    let mut freqs: Vec<u64> = counts.values().copied().collect();
    freqs.sort_unstable_by(|a, b| b.cmp(a));

    // Top-10% of objects should have significantly more requests than bottom-10%
    let top10 = &freqs[..10];
    let bottom10 = &freqs[freqs.len().saturating_sub(10)..];
    let top_sum: u64 = top10.iter().sum();
    let bottom_sum: u64 = bottom10.iter().sum();
    assert!(
        top_sum > bottom_sum * 3,
        "Zipf skew: top-10 ({}) should far exceed bottom-10 ({})",
        top_sum,
        bottom_sum
    );
}

#[test]
fn burst_events_exist_when_probability_nonzero() {
    let config = SyntheticConfig {
        num_requests: 10_000,
        num_objects: 50,
        burst_probability: 0.2, // high burst probability
        burst_size: 5,
        ..SyntheticConfig::default()
    };
    let events = generate(&config).unwrap();

    // Check for consecutive events with the same cache_key and timestamps
    // very close together (within burst_size ms)
    let mut burst_count = 0;
    for w in events.windows(2) {
        if w[0].cache_key == w[1].cache_key {
            let dt = (w[1].timestamp - w[0].timestamp).num_milliseconds();
            if (0..5).contains(&dt) {
                burst_count += 1;
            }
        }
    }
    assert!(
        burst_count > 0,
        "with burst_probability=0.2, bursts should be present"
    );
}

#[test]
fn all_events_have_valid_fields() {
    let config = SyntheticConfig {
        num_requests: 1_000,
        num_objects: 50,
        ..SyntheticConfig::default()
    };
    let events = generate(&config).unwrap();

    for e in &events {
        assert_eq!(e.schema_version, "1.0");
        assert!(!e.object_id.is_empty());
        assert!(!e.cache_key.is_empty());
        assert!(e.object_size_bytes > 0);
        assert!(e.eligible_for_cache);
        assert_eq!(e.status_code, Some(200));
        assert!(e.origin_fetch_cost.is_some());
        assert!(e.response_latency_ms.is_some());
    }
}

#[test]
fn zero_objects_errors() {
    let config = SyntheticConfig {
        num_objects: 0,
        ..SyntheticConfig::default()
    };
    assert!(generate(&config).is_err());
}

#[test]
fn zero_requests_errors() {
    let config = SyntheticConfig {
        num_requests: 0,
        ..SyntheticConfig::default()
    };
    assert!(generate(&config).is_err());
}

#[test]
fn invalid_zipf_alpha_errors() {
    let config = SyntheticConfig {
        zipf_alpha: 0.0,
        ..SyntheticConfig::default()
    };
    assert!(generate(&config).is_err());
}

// ── aggregate_features ──────────────────────────────────────────────

#[test]
fn aggregate_features_counts_correctly() {
    let config = SyntheticConfig {
        num_requests: 5_000,
        num_objects: 50,
        ..SyntheticConfig::default()
    };
    let events = generate(&config).unwrap();
    let features = aggregate_features(&events, config.time_window_seconds);

    // Should have at most num_objects unique features
    assert!(features.len() <= config.num_objects);
    assert!(!features.is_empty());

    // Total request_count across all features should equal num_requests
    let total_requests: u64 = features.iter().map(|f| f.request_count).sum();
    assert_eq!(total_requests, config.num_requests as u64);

    // All features should be eligible
    for f in &features {
        assert!(f.eligible_for_cache);
        assert!(f.request_count > 0);
        assert!(f.request_rate > 0.0);
        assert!(f.size_bytes > 0);
    }
}

#[test]
fn aggregate_features_preserves_avg_values() {
    let config = SyntheticConfig {
        num_requests: 1_000,
        num_objects: 10,
        ..SyntheticConfig::default()
    };
    let events = generate(&config).unwrap();
    let features = aggregate_features(&events, config.time_window_seconds);

    for f in &features {
        assert!(
            f.avg_origin_cost > 0.0,
            "avg_origin_cost should be positive, got {}",
            f.avg_origin_cost
        );
        assert!(
            f.avg_latency_saving_ms > 0.0,
            "avg_latency_saving_ms should be positive, got {}",
            f.avg_latency_saving_ms
        );
    }

    // Verify heterogeneity: not all objects have the same cost
    let costs: Vec<f64> = features.iter().map(|f| f.avg_origin_cost).collect();
    let all_same = costs.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-12);
    assert!(
        !all_same,
        "origin costs should be heterogeneous across objects"
    );
}
