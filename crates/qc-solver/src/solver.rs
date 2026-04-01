use qc_model::object::ScoredObject;
use qc_model::policy::PolicyDecision;
use qc_model::scenario::CapacityConstraint;

use crate::error::SolverError;

#[derive(Debug, Clone)]
pub struct SolverResult {
    pub decisions: Vec<PolicyDecision>,
    pub objective_value: f64,
    pub solve_time_ms: u64,
    pub feasible: bool,
    pub gap: Option<f64>,
    pub shadow_price: Option<f64>,
}

pub trait Solver {
    fn solve(
        &self,
        objects: &[ScoredObject],
        constraint: &CapacityConstraint,
    ) -> Result<SolverResult, SolverError>;
}
