use std::time::Instant;

use qc_model::object::{ScoreBreakdown, ScoredObject};
use qc_model::policy::PolicyDecision;

use crate::error::SolverError;

/// A pairwise interaction between two objects.
#[derive(Debug, Clone)]
pub struct PairwiseInteraction {
    /// Index into the objects array.
    pub i: u32,
    pub j: u32,
    /// Interaction weight: positive = bonus for caching both, negative = penalty.
    pub weight: f64,
}

/// A quadratic optimization problem (QUBO formulation).
///
/// Maximize: Σ linear_i × x_i + Σ J_ij × x_i × x_j
/// Subject to: Σ size_i × x_i ≤ capacity
#[derive(Debug, Clone)]
pub struct QuadraticProblem {
    pub objects: Vec<ScoredObject>,
    pub interactions: Vec<PairwiseInteraction>,
    pub capacity_bytes: u64,
}

/// Result of solving a quadratic problem.
#[derive(Debug, Clone)]
pub struct QuadraticResult {
    pub decisions: Vec<PolicyDecision>,
    pub objective_value: f64,
    pub solve_time_ms: u64,
    pub feasible: bool,
    pub temperature_final: f64,
}

/// Trait for solvers that handle quadratic (QUBO) problems.
pub trait QuadraticSolver {
    fn solve(&self, problem: &QuadraticProblem) -> Result<QuadraticResult, SolverError>;
}

/// Simulated annealing solver for QUBO problems.
pub struct SimulatedAnnealingSolver {
    pub initial_temp: f64,
    pub cooling_rate: f64,
    pub max_iterations: usize,
    pub seed: u64,
}

impl Default for SimulatedAnnealingSolver {
    fn default() -> Self {
        Self {
            initial_temp: 100.0,
            cooling_rate: 0.9995,
            max_iterations: 100_000,
            seed: 42,
        }
    }
}

impl QuadraticSolver for SimulatedAnnealingSolver {
    fn solve(&self, problem: &QuadraticProblem) -> Result<QuadraticResult, SolverError> {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let start = Instant::now();
        let n = problem.objects.len();

        if n == 0 {
            return Ok(QuadraticResult {
                decisions: vec![],
                objective_value: 0.0,
                solve_time_ms: 0,
                feasible: true,
                temperature_final: 0.0,
            });
        }

        let mut rng = StdRng::seed_from_u64(self.seed);

        // Build adjacency for fast interaction lookup
        let mut adj: Vec<Vec<(usize, f64)>> = vec![vec![]; n];
        for inter in &problem.interactions {
            let i = inter.i as usize;
            let j = inter.j as usize;
            if i < n && j < n {
                adj[i].push((j, inter.weight));
                adj[j].push((i, inter.weight));
            }
        }

        // Initialize: greedy start (cache objects with positive linear benefit that fit)
        let mut state: Vec<bool> = vec![false; n];
        let mut used_bytes: u64 = 0;

        // Sort by benefit/size ratio for initial solution
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&a, &b| {
            let ea = if problem.objects[a].size_bytes > 0 {
                problem.objects[a].net_benefit / problem.objects[a].size_bytes as f64
            } else {
                f64::NEG_INFINITY
            };
            let eb = if problem.objects[b].size_bytes > 0 {
                problem.objects[b].net_benefit / problem.objects[b].size_bytes as f64
            } else {
                f64::NEG_INFINITY
            };
            eb.partial_cmp(&ea).unwrap_or(std::cmp::Ordering::Equal)
        });

        for &idx in &order {
            let obj = &problem.objects[idx];
            if obj.net_benefit > 0.0 && used_bytes + obj.size_bytes <= problem.capacity_bytes {
                state[idx] = true;
                used_bytes += obj.size_bytes;
            }
        }

        let mut current_obj = compute_objective(&state, &problem.objects, &adj);
        let mut best_state = state.clone();
        let mut best_obj = current_obj;
        let mut temp = self.initial_temp;

        for _iter in 0..self.max_iterations {
            // Pick a random object to flip
            let idx = rng.gen_range(0..n);
            let obj = &problem.objects[idx];

            // Compute delta
            let new_val = !state[idx];
            let size_delta = if new_val {
                obj.size_bytes as i64
            } else {
                -(obj.size_bytes as i64)
            };
            let new_used = (used_bytes as i64 + size_delta) as u64;

            // Check capacity
            if new_val && new_used > problem.capacity_bytes {
                continue;
            }

            // Compute objective delta
            let linear_delta = if new_val {
                obj.net_benefit
            } else {
                -obj.net_benefit
            };

            let mut quad_delta = 0.0;
            for &(j, w) in &adj[idx] {
                if state[j] {
                    quad_delta += if new_val { w } else { -w };
                }
            }

            let delta = linear_delta + quad_delta;

            // Accept or reject
            let accept = if delta > 0.0 {
                true
            } else {
                rng.gen::<f64>() < (delta / temp).exp()
            };

            if accept {
                state[idx] = new_val;
                used_bytes = new_used;
                current_obj += delta;

                if current_obj > best_obj {
                    best_state = state.clone();
                    best_obj = current_obj;
                }
            }

            temp *= self.cooling_rate;
        }

        // Build decisions from best state
        let decisions: Vec<PolicyDecision> = problem
            .objects
            .iter()
            .zip(best_state.iter())
            .map(|(obj, &cached)| PolicyDecision {
                cache_key: obj.cache_key.clone(),
                cache: cached,
                score: obj.net_benefit,
                score_breakdown: ScoreBreakdown {
                    expected_hit_benefit: obj.score_breakdown.expected_hit_benefit,
                    freshness_cost: obj.score_breakdown.freshness_cost,
                    net_benefit: obj.net_benefit,
                    capacity_shadow_cost: None,
                },
            })
            .collect();

        Ok(QuadraticResult {
            decisions,
            objective_value: best_obj,
            solve_time_ms: start.elapsed().as_millis() as u64,
            feasible: true,
            temperature_final: temp,
        })
    }
}

fn compute_objective(state: &[bool], objects: &[ScoredObject], adj: &[Vec<(usize, f64)>]) -> f64 {
    let mut obj = 0.0;
    for (i, &cached) in state.iter().enumerate() {
        if cached {
            obj += objects[i].net_benefit;
            for &(j, w) in &adj[i] {
                if j > i && state[j] {
                    obj += w;
                }
            }
        }
    }
    obj
}
