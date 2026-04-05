use std::time::Instant;

use good_lp::{
    constraint, variable, Expression, ProblemVariables, Solution, SolverModel, Variable,
};
use qc_model::object::{ScoreBreakdown, ScoredObject};
use qc_model::policy::PolicyDecision;
use qc_model::scenario::CapacityConstraint;

use crate::error::SolverError;
use crate::solver::{Solver, SolverResult};

pub struct ExactIlpSolver;

impl Solver for ExactIlpSolver {
    fn solve(
        &self,
        objects: &[ScoredObject],
        constraint: &CapacityConstraint,
    ) -> Result<SolverResult, SolverError> {
        let start = Instant::now();

        if objects.is_empty() {
            return Ok(SolverResult {
                decisions: vec![],
                objective_value: 0.0,
                solve_time_ms: 0,
                feasible: true,
                gap: None,
                shadow_price: None,
            });
        }

        let mut vars = ProblemVariables::new();
        let xs: Vec<Variable> = vars.add_vector(variable().binary(), objects.len());

        // Objective: maximize sum(benefit_i * x_i)
        let objective: Expression = xs
            .iter()
            .zip(objects.iter())
            .map(|(&x, obj)| x * obj.net_benefit)
            .sum();

        // Capacity constraint: sum(size_i * x_i) <= capacity
        let capacity_expr: Expression = xs
            .iter()
            .zip(objects.iter())
            .map(|(&x, obj)| x * obj.size_bytes as f64)
            .sum();

        // Exclude negative-benefit objects by fixing x_i = 0
        // (good_lp binary is 0..1, but we add benefit <= 0 → x = 0 constraint)
        let mut model = vars.maximise(&objective).using(good_lp::highs);

        model = model.with(constraint!(
            capacity_expr <= constraint.capacity_bytes as f64
        ));

        for (i, obj) in objects.iter().enumerate() {
            if obj.net_benefit <= 0.0 {
                model = model.with(constraint!(xs[i] <= 0.0));
            }
        }

        let solution = model
            .solve()
            .map_err(|e| SolverError::SolverFailure(e.to_string()))?;

        let mut decisions = Vec::with_capacity(objects.len());
        let mut objective_value = 0.0;

        for (i, obj) in objects.iter().enumerate() {
            let val = solution.value(xs[i]);
            let cache = val > 0.5; // binary: 0 or 1
            if cache {
                objective_value += obj.net_benefit;
            }
            decisions.push(PolicyDecision {
                cache_key: obj.cache_key.clone(),
                cache,
                score: obj.net_benefit,
                size_bytes: obj.size_bytes,
                score_breakdown: ScoreBreakdown {
                    expected_hit_benefit: obj.score_breakdown.expected_hit_benefit,
                    freshness_cost: obj.score_breakdown.freshness_cost,
                    net_benefit: obj.net_benefit,
                    capacity_shadow_cost: None,
                },
            });
        }

        let solve_time_ms = start.elapsed().as_millis() as u64;

        Ok(SolverResult {
            decisions,
            objective_value,
            solve_time_ms,
            feasible: true,
            gap: None,
            shadow_price: None,
        })
    }
}
