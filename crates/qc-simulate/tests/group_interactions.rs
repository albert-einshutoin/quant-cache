use qc_model::object::ObjectFeatures;
use qc_model::scenario::StalePenaltyClass;
use qc_simulate::group_interactions::{
    extract_origin_group_interactions, extract_purge_group_interactions,
};

fn make_feature(
    id: &str,
    purge: Option<&str>,
    origin: Option<&str>,
    requests: u64,
) -> ObjectFeatures {
    ObjectFeatures {
        object_id: id.into(),
        cache_key: format!("/{id}"),
        size_bytes: 1024,
        eligible_for_cache: true,
        request_count: requests,
        request_rate: requests as f64 / 86400.0,
        avg_response_bytes: 1024,
        avg_origin_cost: 0.003,
        avg_latency_saving_ms: 50.0,
        ttl_seconds: 3600,
        update_rate: 0.0,
        last_modified: None,
        stale_penalty_class: StalePenaltyClass::Medium,
        purge_group: purge.map(|s| s.into()),
        origin_group: origin.map(|s| s.into()),
        mean_reuse_distance: None,
        reuse_distance_p50: None,
        reuse_distance_p95: None,
    }
}

#[test]
fn empty_features_returns_empty() {
    let pairs = extract_purge_group_interactions(&[], 5.0, 50);
    assert!(pairs.is_empty());
    let pairs = extract_origin_group_interactions(&[], 2.0, 50);
    assert!(pairs.is_empty());
}

#[test]
fn no_groups_returns_empty() {
    let features = vec![
        make_feature("a", None, None, 100),
        make_feature("b", None, None, 200),
    ];
    let pairs = extract_purge_group_interactions(&features, 5.0, 50);
    assert!(pairs.is_empty());
    let pairs = extract_origin_group_interactions(&features, 2.0, 50);
    assert!(pairs.is_empty());
}

#[test]
fn same_purge_group_produces_interaction() {
    let features = vec![
        make_feature("a", Some("pg-1"), None, 100),
        make_feature("b", Some("pg-1"), None, 200),
    ];
    let pairs = extract_purge_group_interactions(&features, 5.0, 50);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].weight, 5.0);
    assert_eq!(pairs[0].count, 300); // 100 + 200
}

#[test]
fn same_origin_group_produces_interaction() {
    let features = vec![
        make_feature("a", None, Some("origin-1"), 100),
        make_feature("b", None, Some("origin-1"), 200),
    ];
    let pairs = extract_origin_group_interactions(&features, 2.0, 50);
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].weight, 2.0);
}

#[test]
fn different_groups_no_interaction() {
    let features = vec![
        make_feature("a", Some("pg-1"), Some("origin-1"), 100),
        make_feature("b", Some("pg-2"), Some("origin-2"), 200),
    ];
    let purge = extract_purge_group_interactions(&features, 5.0, 50);
    assert!(purge.is_empty());
    let origin = extract_origin_group_interactions(&features, 2.0, 50);
    assert!(origin.is_empty());
}

#[test]
fn top_k_truncation() {
    // 4 objects in same group → 6 pairs (4 choose 2), top_k=3
    let features = vec![
        make_feature("a", Some("pg-1"), None, 400),
        make_feature("b", Some("pg-1"), None, 300),
        make_feature("c", Some("pg-1"), None, 200),
        make_feature("d", Some("pg-1"), None, 100),
    ];
    let pairs = extract_purge_group_interactions(&features, 5.0, 3);
    assert_eq!(pairs.len(), 3, "should truncate to top_k=3");
    // Top pairs by count: a+b=700, a+c=600, a+d=500 (or b+c=500)
    assert!(pairs[0].count >= pairs[1].count);
    assert!(pairs[1].count >= pairs[2].count);
}

#[test]
fn ineligible_objects_excluded() {
    let mut features = vec![
        make_feature("a", Some("pg-1"), None, 100),
        make_feature("b", Some("pg-1"), None, 200),
    ];
    features[1].eligible_for_cache = false;
    let pairs = extract_purge_group_interactions(&features, 5.0, 50);
    assert!(
        pairs.is_empty(),
        "ineligible objects should not generate interactions"
    );
}

#[test]
fn mixed_grouped_and_ungrouped() {
    let features = vec![
        make_feature("a", Some("pg-1"), None, 100),
        make_feature("b", Some("pg-1"), None, 200),
        make_feature("c", None, None, 300),         // no group
        make_feature("d", Some("pg-2"), None, 400), // different group
    ];
    let pairs = extract_purge_group_interactions(&features, 5.0, 50);
    assert_eq!(pairs.len(), 1, "only a+b should interact");
}

#[test]
fn single_object_in_group_no_interaction() {
    let features = vec![make_feature("alone", Some("pg-solo"), None, 1000)];
    let pairs = extract_purge_group_interactions(&features, 5.0, 50);
    assert!(pairs.is_empty());
}

// ── Integration: synthetic generator + group assignment ─────────────

#[test]
fn synthetic_assign_groups_populates_fields() {
    let syn_config = qc_simulate::synthetic::SyntheticConfig {
        num_objects: 100,
        num_requests: 5_000,
        seed: 42,
        num_purge_groups: 5,
        num_origin_groups: 3,
        ..Default::default()
    };
    let events = qc_simulate::synthetic::generate(&syn_config).unwrap();
    let mut features =
        qc_simulate::synthetic::aggregate_features_with_options(&events, 86400, false);
    qc_simulate::synthetic::assign_synthetic_groups(&mut features, &syn_config);

    let has_purge = features.iter().filter(|f| f.purge_group.is_some()).count();
    let has_origin = features.iter().filter(|f| f.origin_group.is_some()).count();

    assert_eq!(
        has_purge,
        features.len(),
        "all features should have purge_group"
    );
    assert_eq!(
        has_origin,
        features.len(),
        "all features should have origin_group"
    );

    // Verify group distribution: 5 purge groups, 3 origin groups
    let purge_groups: std::collections::HashSet<_> = features
        .iter()
        .filter_map(|f| f.purge_group.as_ref())
        .collect();
    let origin_groups: std::collections::HashSet<_> = features
        .iter()
        .filter_map(|f| f.origin_group.as_ref())
        .collect();
    assert_eq!(purge_groups.len(), 5, "should have 5 distinct purge groups");
    assert_eq!(
        origin_groups.len(),
        3,
        "should have 3 distinct origin groups"
    );
}

#[test]
fn synthetic_groups_produce_interactions() {
    let syn_config = qc_simulate::synthetic::SyntheticConfig {
        num_objects: 50,
        num_requests: 5_000,
        seed: 99,
        num_purge_groups: 5,
        num_origin_groups: 3,
        ..Default::default()
    };
    let events = qc_simulate::synthetic::generate(&syn_config).unwrap();
    let mut features =
        qc_simulate::synthetic::aggregate_features_with_options(&events, 86400, false);
    qc_simulate::synthetic::assign_synthetic_groups(&mut features, &syn_config);

    let purge = extract_purge_group_interactions(&features, 5.0, 50);
    let origin = extract_origin_group_interactions(&features, 2.0, 50);

    // 50 objects / 5 groups = 10 per group → C(10,2) = 45 pairs per group (capped at 50)
    assert!(!purge.is_empty(), "should produce purge interactions");
    assert!(!origin.is_empty(), "should produce origin interactions");

    // Verify weights
    for p in &purge {
        assert_eq!(p.weight, 5.0);
    }
    for p in &origin {
        assert_eq!(p.weight, 2.0);
    }
}

#[test]
fn no_groups_config_leaves_features_unchanged() {
    let syn_config = qc_simulate::synthetic::SyntheticConfig {
        num_objects: 50,
        num_requests: 5_000,
        seed: 42,
        num_purge_groups: 0,
        num_origin_groups: 0,
        ..Default::default()
    };
    let events = qc_simulate::synthetic::generate(&syn_config).unwrap();
    let mut features =
        qc_simulate::synthetic::aggregate_features_with_options(&events, 86400, false);
    qc_simulate::synthetic::assign_synthetic_groups(&mut features, &syn_config);

    let has_purge = features.iter().filter(|f| f.purge_group.is_some()).count();
    assert_eq!(has_purge, 0, "no groups when config has 0 groups");
}
