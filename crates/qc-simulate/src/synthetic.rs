use chrono::{Duration, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rand_distr::{Distribution, LogNormal, Poisson, Zipf};

use qc_model::trace::RequestTraceEvent;

use crate::error::SimulateError;

/// Configuration for synthetic trace generation.
#[derive(Debug, Clone)]
pub struct SyntheticConfig {
    pub num_objects: usize,
    pub num_requests: usize,
    pub zipf_alpha: f64,
    /// LogNormal parameters for object size (in bytes).
    /// mean = exp(mu + sigma^2/2)
    pub size_log_mu: f64,
    pub size_log_sigma: f64,
    /// Mean update rate (updates/sec) per object, drawn from Poisson.
    pub update_rate_lambda: f64,
    /// Probability that any given request is part of a burst.
    pub burst_probability: f64,
    /// Burst multiplier: how many extra requests a burst generates.
    pub burst_size: usize,
    pub time_window_seconds: u64,
    /// Origin fetch cost per request (fixed for synthetic).
    pub origin_cost: f64,
    /// Average latency saving (ms) for cache hits.
    pub latency_saving_ms: f64,
    /// Random seed for reproducibility.
    pub seed: u64,
    /// Number of distinct purge groups (0 = no purge groups).
    pub num_purge_groups: usize,
    /// Number of distinct origin groups (0 = no origin groups).
    pub num_origin_groups: usize,
}

impl Default for SyntheticConfig {
    fn default() -> Self {
        Self {
            num_objects: 10_000,
            num_requests: 1_000_000,
            zipf_alpha: 0.6,
            size_log_mu: 10.0,
            size_log_sigma: 2.0,
            update_rate_lambda: 0.01,
            burst_probability: 0.05,
            burst_size: 10,
            time_window_seconds: 86400,
            origin_cost: 0.003,
            latency_saving_ms: 50.0,
            seed: 42,
            num_purge_groups: 0,
            num_origin_groups: 0,
        }
    }
}

/// Pre-generated properties for one synthetic object.
#[allow(dead_code)]
struct ObjectProperties {
    object_id: String,
    cache_key: String,
    size_bytes: u64,
    content_type: String,
    origin_cost: f64,
    latency_saving_ms: f64,
    update_rate: f64,
    stale_penalty_class: qc_model::scenario::StalePenaltyClass,
    purge_group: Option<String>,
    origin_group: Option<String>,
}

/// Generate a synthetic trace from the given configuration.
pub fn generate(config: &SyntheticConfig) -> Result<Vec<RequestTraceEvent>, SimulateError> {
    if config.num_objects == 0 || config.num_requests == 0 {
        return Err(SimulateError::GenerationError(
            "num_objects and num_requests must be > 0".into(),
        ));
    }
    if config.zipf_alpha <= 0.0 {
        return Err(SimulateError::GenerationError(
            "zipf_alpha must be > 0".into(),
        ));
    }

    let mut rng = StdRng::seed_from_u64(config.seed);

    // Generate object properties
    let size_dist = LogNormal::new(config.size_log_mu, config.size_log_sigma)
        .map_err(|e| SimulateError::GenerationError(e.to_string()))?;

    // Validate update_rate_lambda for Poisson (used in aggregate_features default)
    let _update_dist = Poisson::new(config.update_rate_lambda)
        .map_err(|e| SimulateError::GenerationError(e.to_string()))?;

    use qc_model::scenario::StalePenaltyClass;

    let content_types = ["text/html", "image/jpeg", "application/json", "text/css"];
    let penalty_classes = [
        StalePenaltyClass::None,
        StalePenaltyClass::Low,
        StalePenaltyClass::Medium,
        StalePenaltyClass::High,
        StalePenaltyClass::VeryHigh,
    ];

    let cost_dist =
        LogNormal::new(-5.0, 1.5).map_err(|e| SimulateError::GenerationError(e.to_string()))?;
    let latency_dist =
        LogNormal::new(3.0, 1.0).map_err(|e| SimulateError::GenerationError(e.to_string()))?;

    let objects: Vec<ObjectProperties> = (0..config.num_objects)
        .map(|i| {
            let size: f64 = size_dist.sample(&mut rng);
            let size_bytes = (size as u64).max(1);
            let ct = content_types[i % content_types.len()];
            let origin_cost: f64 = f64::min(cost_dist.sample(&mut rng), 1.0);
            let latency_saving: f64 = f64::min(latency_dist.sample(&mut rng), 500.0);
            // Draw update_rate from exponential distribution with mean = update_rate_lambda
            let update_rate = if config.update_rate_lambda > 0.0 {
                let exp_sample: f64 = -config.update_rate_lambda * rng.gen::<f64>().ln();
                f64::min(exp_sample, 1.0) // cap at 1 update/sec
            } else {
                0.0
            };
            let penalty = penalty_classes[i % penalty_classes.len()];

            ObjectProperties {
                object_id: format!("obj-{i:06}"),
                cache_key: format!("/content/{i:06}"),
                size_bytes,
                content_type: ct.to_string(),
                origin_cost,
                latency_saving_ms: latency_saving,
                update_rate,
                stale_penalty_class: penalty,
                purge_group: if config.num_purge_groups > 0 {
                    Some(format!("purge-{}", i % config.num_purge_groups))
                } else {
                    None
                },
                origin_group: if config.num_origin_groups > 0 {
                    Some(format!("origin-{}", i % config.num_origin_groups))
                } else {
                    None
                },
            }
        })
        .collect();

    // Generate request sequence using Zipf distribution
    let zipf = Zipf::new(config.num_objects as u64, config.zipf_alpha)
        .map_err(|e| SimulateError::GenerationError(e.to_string()))?;

    if config.time_window_seconds == 0 {
        return Err(SimulateError::GenerationError(
            "time_window_seconds must be > 0".into(),
        ));
    }

    // Use a fixed epoch so traces are deterministic with the same seed.
    let base_time = chrono::DateTime::from_timestamp(0, 0).unwrap_or_else(Utc::now);
    let window_ms = config
        .time_window_seconds
        .checked_mul(1000)
        .ok_or_else(|| SimulateError::GenerationError("time_window_seconds overflow".into()))?;

    // Pre-compute version change schedule per object.
    // Each object gets a list of timestamps when its version changes.
    let mut version_changes: Vec<Vec<i64>> = Vec::with_capacity(config.num_objects);
    for obj in &objects {
        let mut changes = Vec::new();
        if obj.update_rate > 0.0 {
            let mean_interval_ms = (1000.0 / obj.update_rate) as u64;
            if mean_interval_ms > 0 && mean_interval_ms < window_ms {
                let mut t = rng.gen_range(0..mean_interval_ms);
                while t < window_ms {
                    changes.push(t as i64);
                    let next_interval = rng.gen_range(mean_interval_ms / 2..mean_interval_ms * 2);
                    t += next_interval;
                }
            }
        }
        version_changes.push(changes);
    }

    let mut events = Vec::with_capacity(config.num_requests);
    let mut generated = 0usize;

    while generated < config.num_requests {
        // Pick an object (Zipf returns 1-indexed)
        let obj_idx = (zipf.sample(&mut rng) as usize).saturating_sub(1);
        let obj_idx = obj_idx.min(config.num_objects - 1);
        let obj = &objects[obj_idx];

        // Random timestamp within the window
        let offset_ms = rng.gen_range(0..window_ms);
        let timestamp = base_time + Duration::milliseconds(offset_ms as i64);

        // Determine version at this timestamp
        let changes = &version_changes[obj_idx];
        let version_idx = changes.partition_point(|&t| t <= offset_ms as i64);
        let version = format!("v{version_idx}");

        // Determine if this is a burst
        let is_burst = rng.gen::<f64>() < config.burst_probability;
        let repeat = if is_burst { config.burst_size } else { 1 };

        for r in 0..repeat {
            if generated >= config.num_requests {
                break;
            }
            let ts = timestamp + Duration::milliseconds(r as i64);

            events.push(RequestTraceEvent {
                schema_version: RequestTraceEvent::SCHEMA_VERSION.to_string(),
                timestamp: ts,
                object_id: obj.object_id.clone(),
                cache_key: obj.cache_key.clone(),
                object_size_bytes: obj.size_bytes,
                response_bytes: Some(obj.size_bytes),
                cache_status: None,
                status_code: Some(200),
                origin_fetch_cost: Some(obj.origin_cost),
                response_latency_ms: Some(obj.latency_saving_ms),
                region: None,
                content_type: Some(obj.content_type.clone()),
                version_or_etag: Some(version.clone()),
                eligible_for_cache: true,
            });
            generated += 1;
        }
    }

    // Sort by timestamp for replay
    events.sort_by_key(|e| e.timestamp);

    Ok(events)
}

/// Extract `ObjectFeatures` from a generated trace, including reuse distance.
/// Convenience wrapper that always computes reuse distance.
/// For large traces where V2 scoring is not needed, use
/// `aggregate_features_with_options(events, time_window, false)` to skip
/// the O(n·k) reuse distance computation.
pub fn aggregate_features(
    events: &[RequestTraceEvent],
    time_window_seconds: u64,
) -> Vec<qc_model::object::ObjectFeatures> {
    aggregate_features_with_options(events, time_window_seconds, true)
}

/// Extract `ObjectFeatures` from a generated trace.
/// When `compute_reuse` is false, reuse distance fields are left as None
/// (suitable for V1 scoring only).
pub fn aggregate_features_with_options(
    events: &[RequestTraceEvent],
    time_window_seconds: u64,
    compute_reuse: bool,
) -> Vec<qc_model::object::ObjectFeatures> {
    use qc_model::object::ObjectFeatures;
    use qc_model::scenario::StalePenaltyClass;
    use std::collections::{HashMap, HashSet};

    // Compute reuse distances only when requested (V2 scoring)
    let reuse_stats = if compute_reuse {
        crate::reuse_distance::compute_reuse_distances(events)
    } else {
        vec![]
    };
    let reuse_map: HashMap<&str, &crate::reuse_distance::ReuseDistanceStats> = reuse_stats
        .iter()
        .map(|s| (s.cache_key.as_str(), s))
        .collect();

    struct Acc {
        size_bytes: u64,
        request_count: u64,
        total_response_bytes: u64,
        total_origin_cost: f64,
        total_latency_ms: f64,
        eligible: bool,
        versions_seen: HashSet<String>,
        content_type: Option<String>,
    }

    let mut map: HashMap<String, Acc> = HashMap::new();

    for event in events {
        let acc = map.entry(event.cache_key.clone()).or_insert(Acc {
            size_bytes: event.object_size_bytes,
            request_count: 0,
            total_response_bytes: 0,
            total_origin_cost: 0.0,
            total_latency_ms: 0.0,
            eligible: event.eligible_for_cache,
            versions_seen: HashSet::new(),
            content_type: event.content_type.clone(),
        });
        acc.request_count += 1;
        acc.total_response_bytes += event.response_bytes.unwrap_or(event.object_size_bytes);
        acc.total_origin_cost += event.origin_fetch_cost.unwrap_or(0.0);
        acc.total_latency_ms += event.response_latency_ms.unwrap_or(0.0);
        if let Some(ref v) = event.version_or_etag {
            acc.versions_seen.insert(v.clone());
        }
    }

    let tw = if time_window_seconds > 0 {
        time_window_seconds as f64
    } else {
        return vec![];
    };

    map.into_iter()
        .map(|(cache_key, acc)| {
            let rc = acc.request_count.max(1) as f64;
            let avg_cost = acc.total_origin_cost / rc;

            // Estimate update_rate from version diversity
            let version_count = acc.versions_seen.len().max(1) as f64;
            let update_rate = (version_count - 1.0).max(0.0) / tw;

            // Assign stale penalty class based on content type and cost
            let stale_class = match acc.content_type.as_deref() {
                Some("application/json") => {
                    if avg_cost > 0.01 {
                        StalePenaltyClass::VeryHigh
                    } else {
                        StalePenaltyClass::High
                    }
                }
                Some("text/html") => StalePenaltyClass::High,
                Some("text/css") | Some("application/javascript") => StalePenaltyClass::Low,
                Some(ct) if ct.starts_with("image/") || ct.starts_with("video/") => {
                    StalePenaltyClass::None
                }
                _ => StalePenaltyClass::Medium,
            };

            // Populate reuse distance from pre-computed stats
            let rd = reuse_map.get(cache_key.as_str());

            ObjectFeatures {
                object_id: cache_key.clone(),
                cache_key,
                size_bytes: acc.size_bytes,
                eligible_for_cache: acc.eligible,
                request_count: acc.request_count,
                request_rate: rc / tw,
                avg_response_bytes: acc.total_response_bytes / acc.request_count,
                avg_origin_cost: avg_cost,
                avg_latency_saving_ms: acc.total_latency_ms / rc,
                ttl_seconds: 3600,
                update_rate,
                last_modified: None,
                stale_penalty_class: stale_class,
                purge_group: None, // populated by assign_synthetic_groups if needed
                origin_group: None,
                mean_reuse_distance: rd.map(|r| r.mean),
                reuse_distance_p50: rd.map(|r| r.p50),
                reuse_distance_p95: rd.map(|r| r.p95),
            }
        })
        .collect()
}

/// Assign purge_group and origin_group to features based on SyntheticConfig.
///
/// Uses a deterministic mapping from `cache_key` → object index (parsing the
/// numeric suffix from `/content/NNNNNN`), then assigns groups via modular
/// arithmetic matching the `generate()` function's assignment.
///
/// For non-synthetic traces, groups should be populated from external metadata
/// (e.g., config file mapping URL patterns to groups).
pub fn assign_synthetic_groups(
    features: &mut [qc_model::object::ObjectFeatures],
    config: &SyntheticConfig,
) {
    for f in features.iter_mut() {
        // Parse object index from cache_key format "/content/NNNNNN"
        let idx = f
            .cache_key
            .rsplit('/')
            .next()
            .and_then(|s| s.parse::<usize>().ok());

        if let Some(i) = idx {
            if config.num_purge_groups > 0 {
                f.purge_group = Some(format!("purge-{}", i % config.num_purge_groups));
            }
            if config.num_origin_groups > 0 {
                f.origin_group = Some(format!("origin-{}", i % config.num_origin_groups));
            }
        }
    }
}
