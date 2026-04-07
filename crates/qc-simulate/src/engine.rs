use std::collections::HashMap;

use qc_model::compact_trace::CompactTraceEvent;
use qc_model::intern::StringInterner;
use qc_model::metrics::MetricsSummary;
use qc_model::scenario::StalePenaltyClass;
use qc_model::trace::RequestTraceEvent;

use crate::error::SimulateError;

/// Outcome of a single request against a cache policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutcome {
    Hit,
    Miss,
    /// Object served from cache but stale (TTL expired).
    StaleHit,
    /// Request bypassed cache (not eligible).
    Bypass,
}

/// A cache policy that can process trace events one by one.
pub trait CachePolicy {
    fn name(&self) -> &str;

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome;
}

/// Economic evaluation config for replay.
/// Mirrors the scoring parameters so simulator output is comparable to solver objective.
#[derive(Debug, Clone)]
pub struct ReplayEconConfig {
    pub latency_value_per_ms: f64,
    /// Per-object stale penalty ($/event), keyed by cache_key.
    /// Falls back to `default_stale_penalty` if not found.
    pub per_object_stale_penalty: HashMap<String, f64>,
    pub default_stale_penalty: f64,
}

impl Default for ReplayEconConfig {
    fn default() -> Self {
        Self {
            latency_value_per_ms: 0.0,
            per_object_stale_penalty: HashMap::new(),
            default_stale_penalty: 0.0,
        }
    }
}

impl ReplayEconConfig {
    /// Build from ObjectFeatures, using each object's stale_penalty_class.
    pub fn from_features(
        features: &[qc_model::object::ObjectFeatures],
        latency_value_per_ms: f64,
        default_class: StalePenaltyClass,
    ) -> Self {
        Self::from_features_with_overrides(
            features,
            latency_value_per_ms,
            default_class,
            &qc_model::scenario::StaleCostOverrides::default(),
        )
    }

    pub fn from_features_with_overrides(
        features: &[qc_model::object::ObjectFeatures],
        latency_value_per_ms: f64,
        default_class: StalePenaltyClass,
        overrides: &qc_model::scenario::StaleCostOverrides,
    ) -> Self {
        let per_object: HashMap<String, f64> = features
            .iter()
            .map(|f| {
                (
                    f.cache_key.clone(),
                    f.stale_penalty_class.to_cost_with_overrides(overrides),
                )
            })
            .collect();
        Self {
            latency_value_per_ms,
            per_object_stale_penalty: per_object,
            default_stale_penalty: default_class.to_cost_with_overrides(overrides),
        }
    }

    fn stale_penalty_for(&self, cache_key: &str) -> f64 {
        self.per_object_stale_penalty
            .get(cache_key)
            .copied()
            .unwrap_or(self.default_stale_penalty)
    }
}

/// Replays a trace against a cache policy and collects metrics.
pub struct TraceReplayEngine;

impl TraceReplayEngine {
    /// Replay with default (legacy) economic config — origin cost savings only.
    pub fn replay<P: CachePolicy + ?Sized>(
        events: &[RequestTraceEvent],
        policy: &mut P,
    ) -> Result<MetricsSummary, SimulateError> {
        Self::replay_with_econ(events, policy, &ReplayEconConfig::default())
    }

    /// Replay with full economic evaluation matching solver objective.
    pub fn replay_with_econ<P: CachePolicy + ?Sized>(
        events: &[RequestTraceEvent],
        policy: &mut P,
        econ: &ReplayEconConfig,
    ) -> Result<MetricsSummary, SimulateError> {
        if events.is_empty() {
            return Err(SimulateError::EmptyTrace);
        }

        let mut metrics = MetricsSummary::default();

        for event in events {
            let outcome = policy.on_request(event);

            metrics.total_requests += 1;
            let response_bytes = event.response_bytes.unwrap_or(event.object_size_bytes);
            metrics.total_bytes_served += response_bytes;

            let origin_cost = event.origin_fetch_cost.unwrap_or(0.0);
            let latency_ms = event.response_latency_ms.unwrap_or(0.0);

            match outcome {
                CacheOutcome::Hit => {
                    metrics.cache_hits += 1;
                    metrics.bytes_from_cache += response_bytes;
                    metrics.estimated_cost_savings += origin_cost;
                    metrics.policy_objective_value +=
                        latency_ms * econ.latency_value_per_ms + origin_cost;
                }
                CacheOutcome::StaleHit => {
                    metrics.cache_hits += 1;
                    metrics.bytes_from_cache += response_bytes;
                    metrics.stale_serve_count += 1;
                    metrics.estimated_cost_savings += origin_cost;
                    let penalty = econ.stale_penalty_for(&event.cache_key);
                    metrics.policy_objective_value +=
                        latency_ms * econ.latency_value_per_ms + origin_cost - penalty;
                }
                CacheOutcome::Miss => {
                    metrics.cache_misses += 1;
                    metrics.origin_egress_bytes += response_bytes;
                }
                CacheOutcome::Bypass => {
                    metrics.cache_misses += 1;
                    metrics.origin_egress_bytes += response_bytes;
                }
            }
        }

        if metrics.total_requests > 0 {
            metrics.hit_ratio = metrics.cache_hits as f64 / metrics.total_requests as f64;
            metrics.stale_serve_rate =
                metrics.stale_serve_count as f64 / metrics.total_requests as f64;
        }
        if metrics.total_bytes_served > 0 {
            metrics.byte_hit_ratio =
                metrics.bytes_from_cache as f64 / metrics.total_bytes_served as f64;
        }

        Ok(metrics)
    }
}

// ── Compact Replay (u32-keyed) ─────────────────────────────────────

/// A cache policy that operates on compact (interned) trace events.
pub trait CompactCachePolicy {
    fn name(&self) -> &str;

    fn on_request(&mut self, event: &CompactTraceEvent) -> CacheOutcome;
}

/// Economic config for compact replay, keyed by interned u32 IDs.
#[derive(Debug, Clone)]
pub struct CompactReplayEconConfig {
    pub latency_value_per_ms: f64,
    per_object_stale_penalty: HashMap<u32, f64>,
    pub default_stale_penalty: f64,
}

impl CompactReplayEconConfig {
    /// Convert from string-keyed config using the interner.
    pub fn from_econ_config(econ: &ReplayEconConfig, interner: &mut StringInterner) -> Self {
        let per_object: HashMap<u32, f64> = econ
            .per_object_stale_penalty
            .iter()
            .map(|(key, &val)| (interner.intern(key), val))
            .collect();
        Self {
            latency_value_per_ms: econ.latency_value_per_ms,
            per_object_stale_penalty: per_object,
            default_stale_penalty: econ.default_stale_penalty,
        }
    }

    fn stale_penalty_for(&self, cache_key_id: u32) -> f64 {
        self.per_object_stale_penalty
            .get(&cache_key_id)
            .copied()
            .unwrap_or(self.default_stale_penalty)
    }
}

impl TraceReplayEngine {
    /// Replay compact events with economic evaluation.
    pub fn replay_compact_with_econ<P: CompactCachePolicy + ?Sized>(
        events: &[CompactTraceEvent],
        policy: &mut P,
        econ: &CompactReplayEconConfig,
    ) -> Result<MetricsSummary, SimulateError> {
        if events.is_empty() {
            return Err(SimulateError::EmptyTrace);
        }

        let mut metrics = MetricsSummary::default();

        for event in events {
            let outcome = policy.on_request(event);

            metrics.total_requests += 1;
            let response_bytes = event.effective_response_bytes();
            metrics.total_bytes_served += response_bytes;

            let origin_cost = event.origin_fetch_cost;
            let latency_ms = event.response_latency_ms;

            match outcome {
                CacheOutcome::Hit => {
                    metrics.cache_hits += 1;
                    metrics.bytes_from_cache += response_bytes;
                    metrics.estimated_cost_savings += origin_cost;
                    metrics.policy_objective_value +=
                        latency_ms * econ.latency_value_per_ms + origin_cost;
                }
                CacheOutcome::StaleHit => {
                    metrics.cache_hits += 1;
                    metrics.bytes_from_cache += response_bytes;
                    metrics.stale_serve_count += 1;
                    metrics.estimated_cost_savings += origin_cost;
                    let penalty = econ.stale_penalty_for(event.cache_key_id);
                    metrics.policy_objective_value +=
                        latency_ms * econ.latency_value_per_ms + origin_cost - penalty;
                }
                CacheOutcome::Miss => {
                    metrics.cache_misses += 1;
                    metrics.origin_egress_bytes += response_bytes;
                }
                CacheOutcome::Bypass => {
                    metrics.cache_misses += 1;
                    metrics.origin_egress_bytes += response_bytes;
                }
            }
        }

        if metrics.total_requests > 0 {
            metrics.hit_ratio = metrics.cache_hits as f64 / metrics.total_requests as f64;
            metrics.stale_serve_rate =
                metrics.stale_serve_count as f64 / metrics.total_requests as f64;
        }
        if metrics.total_bytes_served > 0 {
            metrics.byte_hit_ratio =
                metrics.bytes_from_cache as f64 / metrics.total_bytes_served as f64;
        }

        Ok(metrics)
    }
}
