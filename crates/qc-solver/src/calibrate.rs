use qc_model::object::ObjectFeatures;
use qc_model::scenario::{
    FreshnessModel, ScenarioConfig, ScoringVersion, StaleCostOverrides, StalePenaltyClass,
    StalePenaltyConfig,
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

/// Train: score + solve → return cached key set.
fn train_policy(
    config: &ScenarioConfig,
    features: &[ObjectFeatures],
    capacity_bytes: u64,
) -> Result<std::collections::HashSet<String>, SolverError> {
    use qc_model::scenario::CapacityConstraint;

    let scored = BenefitCalculator::score_all(features, config)?;
    let constraint = CapacityConstraint { capacity_bytes };
    let result = GreedySolver.solve(&scored, &constraint)?;

    Ok(result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.clone())
        .collect())
}

/// Evaluate a set of cached keys against events (replay proxy).
fn eval_with_keys(
    cached_keys: &std::collections::HashSet<String>,
    events: &[RequestTraceEvent],
    latency_value: f64,
) -> f64 {
    let mut savings = 0.0;
    for event in events {
        if event.eligible_for_cache && cached_keys.contains(&event.cache_key) {
            savings += event.origin_fetch_cost.unwrap_or(0.0);
            savings += event.response_latency_ms.unwrap_or(0.0) * latency_value;
        }
    }
    savings
}

/// Default evaluator: train on given features/events, return score.
/// For proper train/val separation, use `train_policy` + `eval_with_keys`.
pub fn default_eval(
    config: &ScenarioConfig,
    features: &[ObjectFeatures],
    events: &[RequestTraceEvent],
    capacity_bytes: u64,
) -> Result<f64, SolverError> {
    let cached_keys = train_policy(config, features, capacity_bytes)?;
    Ok(eval_with_keys(
        &cached_keys,
        events,
        config.latency_value_per_ms,
    ))
}

/// Calibrate scenario config parameters using coordinate descent.
///
/// Optimizes `latency_value_per_ms` and stale penalty class cost.
/// Uses time-based train/validation split.
pub fn calibrate(
    train_features: &[ObjectFeatures],
    _train_events: &[RequestTraceEvent],
    _val_features: &[ObjectFeatures],
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
                // Train on training data, evaluate policy on validation data
                let cached_keys = train_policy(&config, train_features, capacity)?;
                let val_score =
                    eval_with_keys(&cached_keys, val_events, config.latency_value_per_ms);
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
                let cached_keys = train_policy(&config, train_features, capacity)?;
                let val_score =
                    eval_with_keys(&cached_keys, val_events, config.latency_value_per_ms);
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
        // Train on training data, evaluate on validation data (consistent with main loop)
        let cached_keys = train_policy(&config, train_features, capacity)?;
        let score = eval_with_keys(&cached_keys, val_events, config.latency_value_per_ms);
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
        scoring_version: ScoringVersion::default(),
    }
}
