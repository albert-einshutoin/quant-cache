use qc_model::object::ObjectFeatures;

use crate::co_access::CoAccessPair;

/// Extract pairwise interactions for objects sharing the same `purge_group`.
///
/// Co-caching objects in the same purge group avoids partial-consistency penalties
/// (purging one stale object while its group peers remain cached). Modeled as a
/// positive weight (bonus for co-caching) rather than a penalty for partial caching,
/// because the QUBO `x_i * x_j` term naturally rewards both-cached states.
///
/// Returns at most `top_k_per_group` pairs per group, selecting pairs with the
/// highest combined `request_count` (most impactful pairs).
pub fn extract_purge_group_interactions(
    features: &[ObjectFeatures],
    bonus_weight: f64,
    top_k_per_group: usize,
) -> Vec<CoAccessPair> {
    extract_group_interactions(features, bonus_weight, top_k_per_group, |f| {
        f.purge_group.as_deref()
    })
}

/// Extract pairwise interactions for objects sharing the same `origin_group`.
///
/// Co-caching objects from the same origin reduces burst load on that origin server.
/// A positive weight rewards caching both objects together.
pub fn extract_origin_group_interactions(
    features: &[ObjectFeatures],
    bonus_weight: f64,
    top_k_per_group: usize,
) -> Vec<CoAccessPair> {
    extract_group_interactions(features, bonus_weight, top_k_per_group, |f| {
        f.origin_group.as_deref()
    })
}

fn extract_group_interactions(
    features: &[ObjectFeatures],
    bonus_weight: f64,
    top_k_per_group: usize,
    group_fn: impl Fn(&ObjectFeatures) -> Option<&str>,
) -> Vec<CoAccessPair> {
    use std::collections::HashMap;

    // Group eligible objects by group key
    let mut groups: HashMap<&str, Vec<&ObjectFeatures>> = HashMap::new();
    for f in features {
        if !f.eligible_for_cache {
            continue;
        }
        if let Some(g) = group_fn(f) {
            groups.entry(g).or_default().push(f);
        }
    }

    let mut all_pairs = Vec::new();

    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }

        // For large groups, pre-sort by request_count descending and only pair
        // the top sqrt(2 * top_k) members to avoid O(n²) pair generation.
        let max_members = ((2.0 * top_k_per_group as f64).sqrt().ceil() as usize + 1)
            .max(top_k_per_group)
            .min(members.len());

        let mut sorted_members: Vec<&ObjectFeatures> = members.clone();
        sorted_members.sort_by(|a, b| b.request_count.cmp(&a.request_count));
        let selected = &sorted_members[..max_members];

        let mut group_pairs: Vec<CoAccessPair> = Vec::new();
        for i in 0..selected.len() {
            for j in (i + 1)..selected.len() {
                let a = selected[i];
                let b = selected[j];
                let (key_a, key_b) = if a.cache_key < b.cache_key {
                    (a.cache_key.clone(), b.cache_key.clone())
                } else {
                    (b.cache_key.clone(), a.cache_key.clone())
                };
                let count = a.request_count + b.request_count;
                group_pairs.push(CoAccessPair {
                    key_a,
                    key_b,
                    count,
                    weight: bonus_weight,
                });
            }
        }

        // Keep top-K pairs by combined request_count
        group_pairs.sort_by(|a, b| b.count.cmp(&a.count));
        group_pairs.truncate(top_k_per_group);
        all_pairs.extend(group_pairs);
    }

    all_pairs
}
