use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scenario::StalePenaltyClass;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectFeatures {
    pub object_id: String,
    pub cache_key: String,
    pub size_bytes: u64,
    pub eligible_for_cache: bool,
    pub request_count: u64,
    pub request_rate: f64,
    pub avg_response_bytes: u64,
    pub avg_origin_cost: f64,
    pub avg_latency_saving_ms: f64,
    /// TTL is a fixed input in V1 (not optimized).
    pub ttl_seconds: u64,
    pub update_rate: f64,
    pub last_modified: Option<DateTime<Utc>>,
    pub stale_penalty_class: StalePenaltyClass,
    pub purge_group: Option<String>,
    pub origin_group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredObject {
    pub object_id: String,
    pub cache_key: String,
    pub size_bytes: u64,
    pub net_benefit: f64,
    pub score_breakdown: ScoreBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub expected_hit_benefit: f64,
    pub freshness_cost: f64,
    pub net_benefit: f64,
    /// Diagnostic value (not an optimization term).
    /// Greedy cutoff μ* × size.
    pub capacity_shadow_cost: Option<f64>,
}
