use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsSummary {
    // Primary metrics
    pub total_requests: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub hit_ratio: f64,
    pub total_bytes_served: u64,
    pub bytes_from_cache: u64,
    pub byte_hit_ratio: f64,
    pub origin_egress_bytes: u64,
    pub estimated_cost_savings: f64,
    pub policy_objective_value: f64,

    // Diagnostic metrics
    pub stale_serve_count: u64,
    pub stale_serve_rate: f64,
    pub policy_churn: f64,
    pub solve_time_ms: u64,
    pub capacity_utilization: f64,
    pub optimality_gap: Option<f64>,
}
