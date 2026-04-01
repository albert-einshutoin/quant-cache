use qc_model::object::{ObjectFeatures, ScoreBreakdown, ScoredObject};
use qc_model::scenario::{FreshnessModel, ScenarioConfig};

use crate::error::SolverError;

pub struct BenefitCalculator;

impl BenefitCalculator {
    pub fn score(
        object: &ObjectFeatures,
        config: &ScenarioConfig,
    ) -> Result<ScoredObject, SolverError> {
        if !object.eligible_for_cache {
            return Ok(ScoredObject {
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
            });
        }

        let expected_requests = object.request_count as f64;

        let expected_hit_benefit = expected_requests
            * (object.avg_latency_saving_ms * config.latency_value_per_ms + object.avg_origin_cost);

        let freshness_cost = match &config.freshness_model {
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
        };

        let net_benefit = expected_hit_benefit - freshness_cost;

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

    pub fn score_all(
        objects: &[ObjectFeatures],
        config: &ScenarioConfig,
    ) -> Result<Vec<ScoredObject>, SolverError> {
        objects.iter().map(|o| Self::score(o, config)).collect()
    }
}
