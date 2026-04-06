use std::collections::HashMap;

use qc_model::trace::RequestTraceEvent;

/// Co-access pair with weight.
#[derive(Debug, Clone)]
pub struct CoAccessPair {
    pub key_a: String,
    pub key_b: String,
    pub count: u64,
    pub weight: f64,
}

/// Extract co-access pairs from a trace using time-window co-occurrence.
///
/// Two objects co-occur if they are both accessed within `window_ms` of each other.
/// Returns pairs sorted by count descending, limited to `top_k`.
pub fn extract_co_access(
    events: &[RequestTraceEvent],
    window_ms: i64,
    top_k: usize,
) -> Vec<CoAccessPair> {
    let eligible: Vec<&RequestTraceEvent> =
        events.iter().filter(|e| e.eligible_for_cache).collect();

    const MAX_INNER_PAIRS: usize = 50_000_000;

    let mut pair_counts: HashMap<(String, String), u64> = HashMap::new();
    let mut total_pairs: usize = 0;

    'outer: for (i, a) in eligible.iter().enumerate() {
        let a_time = a.timestamp;
        for b in &eligible[i + 1..] {
            let diff = (b.timestamp - a_time).num_milliseconds();
            if diff > window_ms {
                break;
            }
            if a.cache_key == b.cache_key {
                continue;
            }
            let pair = if a.cache_key < b.cache_key {
                (a.cache_key.clone(), b.cache_key.clone())
            } else {
                (b.cache_key.clone(), a.cache_key.clone())
            };
            *pair_counts.entry(pair).or_insert(0) += 1;
            total_pairs += 1;
            if total_pairs >= MAX_INNER_PAIRS {
                tracing::warn!("co_access: pair limit ({MAX_INNER_PAIRS}) reached, truncating");
                break 'outer;
            }
        }
    }

    let max_count = pair_counts.values().copied().max().unwrap_or(1) as f64;

    let mut pairs: Vec<CoAccessPair> = pair_counts
        .into_iter()
        .map(|((key_a, key_b), count)| CoAccessPair {
            key_a,
            key_b,
            count,
            weight: count as f64 / max_count, // normalized to [0, 1]
        })
        .collect();

    pairs.sort_by(|a, b| b.count.cmp(&a.count));
    pairs.truncate(top_k);
    pairs
}
