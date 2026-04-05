use std::time::Instant;

use qc_model::object::{ScoreBreakdown, ScoredObject};
use qc_model::policy::PolicyDecision;
use qc_model::scenario::CapacityConstraint;

use crate::error::SolverError;
use crate::solver::{Solver, SolverResult};

pub struct GreedySolver;

impl Solver for GreedySolver {
    fn solve(
        &self,
        objects: &[ScoredObject],
        constraint: &CapacityConstraint,
    ) -> Result<SolverResult, SolverError> {
        let start = Instant::now();

        let ratio_result = solve_by_ratio(objects, constraint);
        let pure_result = solve_by_benefit(objects, constraint);

        let (objective_value, decisions, shadow_price) = if ratio_result.0 >= pure_result.0 {
            ratio_result
        } else {
            pure_result
        };

        let solve_time_ms = start.elapsed().as_millis() as u64;

        Ok(SolverResult {
            decisions,
            objective_value,
            solve_time_ms,
            feasible: true,
            gap: None,
            shadow_price: Some(shadow_price),
        })
    }
}

/// Sort by benefit/size ratio (efficiency).
fn solve_by_ratio(
    objects: &[ScoredObject],
    constraint: &CapacityConstraint,
) -> (f64, Vec<PolicyDecision>, f64) {
    let mut indices: Vec<usize> = (0..objects.len()).collect();
    indices.sort_by(|&a, &b| {
        let eff_a = if objects[a].size_bytes > 0 {
            objects[a].net_benefit / objects[a].size_bytes as f64
        } else {
            f64::NEG_INFINITY
        };
        let eff_b = if objects[b].size_bytes > 0 {
            objects[b].net_benefit / objects[b].size_bytes as f64
        } else {
            f64::NEG_INFINITY
        };
        eff_b
            .partial_cmp(&eff_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    fill_knapsack(objects, &indices, constraint)
}

/// Sort by pure benefit (absolute value).
fn solve_by_benefit(
    objects: &[ScoredObject],
    constraint: &CapacityConstraint,
) -> (f64, Vec<PolicyDecision>, f64) {
    let mut indices: Vec<usize> = (0..objects.len()).collect();
    indices.sort_by(|&a, &b| {
        objects[b]
            .net_benefit
            .partial_cmp(&objects[a].net_benefit)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    fill_knapsack(objects, &indices, constraint)
}

fn fill_knapsack(
    objects: &[ScoredObject],
    sorted_indices: &[usize],
    constraint: &CapacityConstraint,
) -> (f64, Vec<PolicyDecision>, f64) {
    let mut used_bytes: u64 = 0;
    let mut objective = 0.0;
    let mut shadow_price = 0.0;
    let mut decisions = Vec::with_capacity(objects.len());
    let mut cutoff_found = false;

    for &idx in sorted_indices {
        let obj = &objects[idx];

        if obj.net_benefit <= 0.0 {
            decisions.push(make_decision(obj, false));
            continue;
        }

        if used_bytes + obj.size_bytes <= constraint.capacity_bytes {
            used_bytes += obj.size_bytes;
            objective += obj.net_benefit;
            decisions.push(make_decision(obj, true));
        } else {
            if !cutoff_found {
                shadow_price = if obj.size_bytes > 0 {
                    obj.net_benefit / obj.size_bytes as f64
                } else {
                    0.0
                };
                cutoff_found = true;
            }
            decisions.push(make_decision(obj, false));
        }
    }

    (objective, decisions, shadow_price)
}

fn make_decision(obj: &ScoredObject, cache: bool) -> PolicyDecision {
    PolicyDecision {
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
    }
}
