//! Scoring, optimization, and policy search engine for quant-cache.
//!
//! Provides:
//! - [`score::BenefitCalculator`] / [`score::Scorer`] trait — V1 (frequency) and V2 (reuse-distance) scoring
//! - [`greedy::GreedySolver`] — O(n log n) knapsack heuristic
//! - [`ilp::ExactIlpSolver`] — exact ILP via HiGHS
//! - [`qubo::SimulatedAnnealingSolver`] — SA for quadratic problems with co-access interactions
//! - [`policy_search`] — grid, SA, and QUBO search over PolicyIR DSL space
//! - [`calibrate`] — coordinate descent calibration of economic parameters

pub mod calibrate;
pub mod error;
pub mod greedy;
pub mod ilp;
pub mod policy_qubo;
pub mod policy_search;
pub mod qubo;
pub mod score;
pub mod solver;
