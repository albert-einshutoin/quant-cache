use std::collections::HashMap;

use qc_model::trace::RequestTraceEvent;

/// Per-object reuse distance statistics.
#[derive(Debug, Clone)]
pub struct ReuseDistanceStats {
    pub cache_key: String,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub sample_count: usize,
}

/// Compute reuse distance distribution for each cache_key in the trace.
///
/// Reuse distance = number of distinct cache_keys accessed between two consecutive
/// accesses to the same key (stack distance).
pub fn compute_reuse_distances(events: &[RequestTraceEvent]) -> Vec<ReuseDistanceStats> {
    // Track last access position per key
    let mut last_seen: HashMap<&str, usize> = HashMap::new();
    // Track the set of distinct keys since last access (approximated via position diff)
    let mut distances: HashMap<String, Vec<usize>> = HashMap::new();

    // We approximate reuse distance as the number of distinct keys accessed
    // between consecutive accesses to the same key.
    // For efficiency, we use an ordered access log and count unique keys in the window.
    let access_order: Vec<&str> = events
        .iter()
        .filter(|e| e.eligible_for_cache)
        .map(|e| e.cache_key.as_str())
        .collect();

    for (pos, &key) in access_order.iter().enumerate() {
        if let Some(&prev_pos) = last_seen.get(key) {
            let mut seen_in_window: std::collections::HashSet<&str> =
                std::collections::HashSet::new();
            let start = (prev_pos + 1).min(pos);
            for &k in &access_order[start..pos] {
                if k != key {
                    seen_in_window.insert(k);
                }
            }
            let distinct = seen_in_window.len();
            distances.entry(key.to_string()).or_default().push(distinct);
        }
        last_seen.insert(key, pos);
    }

    distances
        .into_iter()
        .map(|(cache_key, mut dists)| {
            dists.sort_unstable();
            let n = dists.len();
            let mean = if n > 0 {
                dists.iter().sum::<usize>() as f64 / n as f64
            } else {
                f64::INFINITY
            };
            let p50 = if n > 0 {
                dists[n / 2] as f64
            } else {
                f64::INFINITY
            };
            let p95 = if n > 0 {
                dists[(n * 95 / 100).min(n - 1)] as f64
            } else {
                f64::INFINITY
            };

            ReuseDistanceStats {
                cache_key,
                mean,
                p50,
                p95,
                sample_count: n,
            }
        })
        .collect()
}
