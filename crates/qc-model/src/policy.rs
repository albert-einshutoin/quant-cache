use serde::{Deserialize, Serialize};

use crate::object::ScoreBreakdown;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub cache_key: String,
    pub cache: bool,
    pub score: f64,
    #[serde(default)]
    pub size_bytes: u64,
    pub score_breakdown: ScoreBreakdown,
}

/// Wrapper for policy JSON output, including solver metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyFile {
    pub solver: SolverMetadata,
    pub decisions: Vec<PolicyDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverMetadata {
    pub solver_name: String,
    pub objective_value: f64,
    pub solve_time_ms: u64,
    pub shadow_price: Option<f64>,
    pub optimality_gap: Option<f64>,
    pub capacity_bytes: u64,
    pub cached_bytes: u64,
}
