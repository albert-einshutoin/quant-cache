use qc_model::object::ObjectFeatures;
use qc_model::scenario::{
    FreshnessModel, ScenarioConfig, StaleCostOverrides, StalePenaltyClass, StalePenaltyConfig,
};
use qc_model::trace::RequestTraceEvent;

use crate::error::SolverError;
use crate::greedy::GreedySolver;
use crate::score::BenefitCalculator;
use crate::solver::Solver;

/// Result of coefficient calibration.
#[derive(Debug, Clone)]
pub struct CalibrationResult {
    pub best_config: ScenarioConfig,
    pub best_score: f64,
    pub iterations: usize,
    pub parameter_sensitivity: Vec<(String, f64, f64)>, // (name, best_value, sensitivity)
}

/// Evaluator function: given a config, features, and events, returns a score to maximize.
pub type EvalFn = Box<
    dyn Fn(
        &ScenarioConfig,
        &[ObjectFeatures],
        &[RequestTraceEvent],
        u64,
    ) -> Result<f64, SolverError>,
>;

/// Default evaluator: optimize → replay → estimated_cost_savings.
pub fn default_eval(
    config: &ScenarioConfig,
    features: &[ObjectFeatures],
    events: &[RequestTraceEvent],
    capacity_bytes: u64,
) -> Result<f64, SolverError> {
    use qc_model::scenario::CapacityConstraint;

    let scored = BenefitCalculator::score_all(features, config)?;
    let constraint = CapacityConstraint { capacity_bytes };
    let result = GreedySolver.solve(&scored, &constraint)?;

    let cached_keys: std::collections::HashSet<String> = result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.clone())
        .collect();

    // Simple replay: count cost savings
    let mut savings = 0.0;
    for event in events {
        if event.eligible_for_cache && cached_keys.contains(&event.cache_key) {
            savings += event.origin_fetch_cost.unwrap_or(0.0);
        }
    }

    Ok(savings)
}

/// Calibrate scenario config parameters using coordinate descent.
///
/// Optimizes `latency_value_per_ms` and stale penalty class cost.
/// Uses time-based train/validation split.
pub fn calibrate(
    train_features: &[ObjectFeatures],
    train_events: &[RequestTraceEvent],
    val_features: &[ObjectFeatures],
    val_events: &[RequestTraceEvent],
    base_config: &ScenarioConfig,
    num_restarts: usize,
) -> Result<CalibrationResult, SolverError> {
    let capacity = base_config.capacity_bytes;

    // Parameter ranges
    let latency_range = [0.00001, 0.00002, 0.00005, 0.0001, 0.0002, 0.0005, 0.001];
    let penalty_range = [
        StalePenaltyClass::None,
        StalePenaltyClass::Low,
        StalePenaltyClass::Medium,
        StalePenaltyClass::High,
        StalePenaltyClass::VeryHigh,
    ];

    let mut best_config = base_config.clone();
    let mut best_val_score = f64::NEG_INFINITY;
    let mut total_iters = 0;

    for restart in 0..num_restarts.max(1) {
        // Start from different initial points
        let init_lat_idx = restart % latency_range.len();
        let init_pen_idx = restart % penalty_range.len();

        let mut current_lat = latency_range[init_lat_idx];
        let mut current_pen = penalty_range[init_pen_idx];
        let mut improved = true;

        while improved {
            improved = false;

            // Optimize latency_value_per_ms
            for &lat in &latency_range {
                let config = make_config(capacity, lat, current_pen);
                // Train to get policy, validate to score
                let _ = default_eval(&config, train_features, train_events, capacity)?;
                let val_score = default_eval(&config, val_features, val_events, capacity)?;
                total_iters += 1;

                if val_score > best_val_score {
                    best_val_score = val_score;
                    best_config = config;
                    current_lat = lat;
                    improved = true;
                }
            }

            // Optimize stale penalty class
            for &pen in &penalty_range {
                let config = make_config(capacity, current_lat, pen);
                let _ = default_eval(&config, train_features, train_events, capacity)?;
                let val_score = default_eval(&config, val_features, val_events, capacity)?;
                total_iters += 1;

                if val_score > best_val_score {
                    best_val_score = val_score;
                    best_config = config;
                    current_pen = pen;
                    improved = true;
                }
            }
        }
    }

    // Compute sensitivity
    let mut sensitivity = Vec::new();
    let base_score = best_val_score;

    for &lat in &latency_range {
        let pen = match &best_config.freshness_model {
            FreshnessModel::TtlOnly { stale_penalty } => stale_penalty.default_class,
            _ => StalePenaltyClass::Medium,
        };
        let config = make_config(capacity, lat, pen);
        let score = default_eval(&config, val_features, val_events, capacity)?;
        sensitivity.push(("latency_value_per_ms".into(), lat, score - base_score));
    }

    Ok(CalibrationResult {
        best_config,
        best_score: best_val_score,
        iterations: total_iters,
        parameter_sensitivity: sensitivity,
    })
}

fn make_config(
    capacity: u64,
    latency_value: f64,
    penalty_class: StalePenaltyClass,
) -> ScenarioConfig {
    ScenarioConfig {
        capacity_bytes: capacity,
        time_window_seconds: 86400,
        latency_value_per_ms: latency_value,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: penalty_class,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
    }
}
