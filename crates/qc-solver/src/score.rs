use qc_model::object::{ObjectFeatures, ScoreBreakdown, ScoredObject};
use qc_model::scenario::{FreshnessModel, ScenarioConfig, ScoringVersion};

use crate::error::SolverError;

/// Trait for scoring strategies. Implement this to add new scoring versions.
pub trait Scorer {
    /// Score a single object.
    fn score(
        &self,
        object: &ObjectFeatures,
        config: &ScenarioConfig,
    ) -> Result<ScoredObject, SolverError>;

    /// Batch-score all objects. Default implementation calls `score` per object.
    fn score_all(
        &self,
        objects: &[ObjectFeatures],
        config: &ScenarioConfig,
    ) -> Result<Vec<ScoredObject>, SolverError> {
        objects.iter().map(|o| self.score(o, config)).collect()
    }
}

/// V1: frequency-based demand estimation.
pub struct V1Scorer;

impl Scorer for V1Scorer {
    fn score(
        &self,
        object: &ObjectFeatures,
        config: &ScenarioConfig,
    ) -> Result<ScoredObject, SolverError> {
        BenefitCalculator::score_v1(object, config)
    }
}

/// V2: reuse-distance-aware demand estimation.
/// Requires a global mean object size for consistent capacity estimates.
pub struct V2Scorer {
    pub mean_object_size: f64,
}

impl Scorer for V2Scorer {
    fn score(
        &self,
        object: &ObjectFeatures,
        config: &ScenarioConfig,
    ) -> Result<ScoredObject, SolverError> {
        BenefitCalculator::score_v2(object, config, self.mean_object_size)
    }
}

/// Create the appropriate scorer for a config. For V2, computes global mean object size.
pub fn create_scorer(objects: &[ObjectFeatures], config: &ScenarioConfig) -> Box<dyn Scorer> {
    match config.scoring_version {
        ScoringVersion::V1Frequency => Box::new(V1Scorer),
        ScoringVersion::V2ReuseDistance => {
            let eligible: Vec<&ObjectFeatures> =
                objects.iter().filter(|o| o.eligible_for_cache).collect();
            let mean_size = if eligible.is_empty() {
                1.0
            } else {
                eligible.iter().map(|o| o.size_bytes as f64).sum::<f64>() / eligible.len() as f64
            };
            Box::new(V2Scorer {
                mean_object_size: mean_size,
            })
        }
    }
}

/// Convenience façade — delegates to the appropriate Scorer based on config.
/// Backward-compatible with existing callers.
pub struct BenefitCalculator;

impl BenefitCalculator {
    /// Score a single object.
    pub fn score(
        object: &ObjectFeatures,
        config: &ScenarioConfig,
    ) -> Result<ScoredObject, SolverError> {
        match config.scoring_version {
            ScoringVersion::V1Frequency => Self::score_v1(object, config),
            ScoringVersion::V2ReuseDistance => {
                let mean_size = object.size_bytes as f64;
                Self::score_v2(object, config, mean_size)
            }
        }
    }

    /// Batch-score all objects.
    pub fn score_all(
        objects: &[ObjectFeatures],
        config: &ScenarioConfig,
    ) -> Result<Vec<ScoredObject>, SolverError> {
        let scorer = create_scorer(objects, config);
        scorer.score_all(objects, config)
    }

    /// V1 scoring: frequency-based demand estimation.
    /// expected_hits = request_count (all requests assumed to hit if cached).
    pub(crate) fn score_v1(
        object: &ObjectFeatures,
        config: &ScenarioConfig,
    ) -> Result<ScoredObject, SolverError> {
        if !object.eligible_for_cache {
            return Ok(Self::zero_score(object));
        }

        let expected_requests = object.request_count as f64;

        let expected_hit_benefit = expected_requests
            * (object.avg_latency_saving_ms * config.latency_value_per_ms + object.avg_origin_cost);

        let freshness_cost = Self::compute_freshness_cost(object, config, expected_requests);
        let net_benefit = expected_hit_benefit - freshness_cost;
        // Clamp non-finite values (NaN/Inf from malformed input) to 0.
        let net_benefit = if net_benefit.is_finite() {
            net_benefit
        } else {
            0.0
        };

        Ok(ScoredObject {
            object_id: object.object_id.clone(),
            cache_key: object.cache_key.clone(),
            size_bytes: object.size_bytes,
            net_benefit,
            score_breakdown: ScoreBreakdown {
                expected_hit_benefit,
                freshness_cost,
                net_benefit,
                capacity_shadow_cost: None,
            },
        })
    }

    /// V2 scoring: reuse-distance-aware demand estimation.
    ///
    /// Instead of treating all requests as hits, we estimate hit probability
    /// from the reuse distance distribution. Objects with low reuse distance
    /// (high temporal locality) are more likely to remain in cache between
    /// accesses, thus have higher expected hit rates.
    ///
    /// Hit probability model:
    ///   p_hit = exp(-reuse_distance_p50 / cache_capacity_objects)
    ///
    /// where cache_capacity_objects = capacity_bytes / mean_object_size.
    /// Using a global mean object size (computed in `score_all`) ensures all
    /// objects share the same cache capacity estimate, avoiding size-based
    /// double-penalization with the knapsack solver's benefit/size ratio.
    ///
    /// Falls back to V1 scoring when reuse distance data is not available.
    /// A reuse_distance_p50 of 0.0 is valid (sequential repeated access)
    /// and produces p_hit = 1.0.
    pub(crate) fn score_v2(
        object: &ObjectFeatures,
        config: &ScenarioConfig,
        mean_object_size: f64,
    ) -> Result<ScoredObject, SolverError> {
        if !object.eligible_for_cache {
            return Ok(Self::zero_score(object));
        }

        // Fall back to V1 if reuse distance data is missing or infinite
        // (infinite means the object was only accessed once — no reuse observed).
        let rd_p50 = match object.reuse_distance_p50 {
            Some(p50) if p50.is_finite() => p50,
            _ => return Self::score_v1(object, config),
        };

        let expected_requests = object.request_count as f64;

        // Estimate cache capacity in number of objects using the global mean size.
        let mean_size = if mean_object_size > 0.0 {
            mean_object_size
        } else {
            1.0
        };
        let cache_capacity_objects = config.capacity_bytes as f64 / mean_size;

        // Guard: zero capacity means nothing can be cached → p_hit = 0
        if cache_capacity_objects <= 0.0 {
            return Ok(Self::zero_score(object));
        }

        // Hit probability: exponential decay based on reuse distance relative to cache size.
        // p_hit = exp(-rd_p50 / C_objects)
        // - rd_p50 = 0 → p_hit = 1.0 (always in cache, valid for sequential access)
        // - rd_p50 = C_objects → p_hit ≈ 0.37 (marginal)
        // - rd_p50 >> C_objects → p_hit → 0 (evicted before re-access)
        let p_hit = (-rd_p50 / cache_capacity_objects).exp();

        let expected_hits = expected_requests * p_hit;

        let expected_hit_benefit = expected_hits
            * (object.avg_latency_saving_ms * config.latency_value_per_ms + object.avg_origin_cost);

        // V2 intentionally passes expected_hits (not expected_requests) for TtlOnly:
        // stale content can only be served on cache hits, so stale cost scales with p_hit.
        // For InvalidationOnUpdate, the parameter is unused (cost is per-update).
        let freshness_cost = Self::compute_freshness_cost(object, config, expected_hits);
        let net_benefit = expected_hit_benefit - freshness_cost;
        let net_benefit = if net_benefit.is_finite() {
            net_benefit
        } else {
            0.0
        };

        Ok(ScoredObject {
            object_id: object.object_id.clone(),
            cache_key: object.cache_key.clone(),
            size_bytes: object.size_bytes,
            net_benefit,
            score_breakdown: ScoreBreakdown {
                expected_hit_benefit,
                freshness_cost,
                net_benefit,
                capacity_shadow_cost: None,
            },
        })
    }

    fn zero_score(object: &ObjectFeatures) -> ScoredObject {
        ScoredObject {
            object_id: object.object_id.clone(),
            cache_key: object.cache_key.clone(),
            size_bytes: object.size_bytes,
            net_benefit: 0.0,
            score_breakdown: ScoreBreakdown {
                expected_hit_benefit: 0.0,
                freshness_cost: 0.0,
                net_benefit: 0.0,
                capacity_shadow_cost: None,
            },
        }
    }

    fn compute_freshness_cost(
        object: &ObjectFeatures,
        config: &ScenarioConfig,
        expected_requests: f64,
    ) -> f64 {
        match &config.freshness_model {
            FreshnessModel::TtlOnly { stale_penalty } => {
                let penalty = object
                    .stale_penalty_class
                    .to_cost_with_overrides(&stale_penalty.cost_overrides);
                let p_stale = 1.0 - (-object.update_rate * object.ttl_seconds as f64).exp();
                expected_requests * p_stale * penalty
            }
            FreshnessModel::InvalidationOnUpdate { invalidation_cost } => {
                let expected_invalidations = object.update_rate * config.time_window_seconds as f64;
                expected_invalidations * invalidation_cost
            }
        }
    }
}
