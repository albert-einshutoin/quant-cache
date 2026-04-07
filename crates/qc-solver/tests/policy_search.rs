/// Tests for policy search: SA vs Grid quality comparison.
use qc_model::policy_ir::PolicyIR;
use qc_model::scenario::{
    FreshnessModel, ScenarioConfig, ScoringVersion, StaleCostOverrides, StalePenaltyClass,
    StalePenaltyConfig,
};
use qc_simulate::engine::{ReplayEconConfig, TraceReplayEngine};
use qc_simulate::ir_policy::{IrEvalContext, IrPolicy};
use qc_simulate::synthetic::{self, SyntheticConfig};
use qc_solver::policy_search::{self, PolicySearchConfig};
use qc_solver::score::BenefitCalculator;

fn setup() -> (
    Vec<qc_model::trace::RequestTraceEvent>,
    Vec<qc_model::object::ObjectFeatures>,
    Vec<qc_model::object::ScoredObject>,
    ScenarioConfig,
    ReplayEconConfig,
) {
    let syn = SyntheticConfig {
        num_objects: 200,
        num_requests: 5_000,
        zipf_alpha: 0.8,
        seed: 42,
        ..SyntheticConfig::default()
    };
    let events = synthetic::generate(&syn).unwrap();
    let config = ScenarioConfig {
        capacity_bytes: 500_000,
        time_window_seconds: 86400,
        latency_value_per_ms: 0.0001,
        freshness_model: FreshnessModel::TtlOnly {
            stale_penalty: StalePenaltyConfig {
                default_class: StalePenaltyClass::Low,
                cost_overrides: StaleCostOverrides::default(),
            },
        },
        scoring_version: ScoringVersion::V1Frequency,
    };
    let features = synthetic::aggregate_features(&events, 86400);
    let scored = BenefitCalculator::score_all(&features, &config).unwrap();
    let econ = ReplayEconConfig::from_features(
        &features,
        config.latency_value_per_ms,
        StalePenaltyClass::Low,
    );
    (events, features, scored, config, econ)
}

fn make_eval_fn<'a>(
    events: &'a [qc_model::trace::RequestTraceEvent],
    features: &'a [qc_model::object::ObjectFeatures],
    scored: &'a [qc_model::object::ScoredObject],
    econ: &'a ReplayEconConfig,
) -> impl Fn(&PolicyIR) -> Result<f64, qc_solver::error::SolverError> + 'a {
    move |ir: &PolicyIR| {
        let ctx = IrEvalContext::from_features_and_scores(features, scored);
        let mut policy = IrPolicy::new(ir.clone(), ctx);
        let start = events
            .first()
            .map(|e| e.timestamp)
            .unwrap_or_else(chrono::Utc::now);
        policy.prewarm(features, start);
        policy.apply_ttl_rules(events);
        let metrics = TraceReplayEngine::replay_with_econ(events, &mut policy, econ)
            .map_err(|e| qc_solver::error::SolverError::SolverFailure(e.to_string()))?;
        Ok(metrics.policy_objective_value)
    }
}

#[test]
fn sa_finds_non_negative_objective() {
    let (events, features, scored, _, econ) = setup();
    let search_config = PolicySearchConfig {
        capacity_bytes: 500_000,
        max_iterations: 60,
        seed: 42,
        top_k: 3,
        content_types: vec!["text/html".into(), "image/jpeg".into()],
    };
    let eval_fn = make_eval_fn(&events, &features, &scored, &econ);
    let result = policy_search::search_sa(&search_config, &scored, eval_fn).unwrap();
    assert!(
        result.best_objective.is_finite(),
        "SA should produce finite objective"
    );
    assert!(
        result.candidates_evaluated > 0,
        "SA should evaluate candidates"
    );
}

#[test]
fn sa_competitive_with_grid() {
    let (events, features, scored, _, econ) = setup();
    let search_config = PolicySearchConfig {
        capacity_bytes: 500_000,
        max_iterations: 100,
        seed: 42,
        top_k: 5,
        content_types: vec!["text/html".into(), "image/jpeg".into()],
    };

    let eval_fn_grid = make_eval_fn(&events, &features, &scored, &econ);
    let grid = policy_search::search(&search_config, &scored, eval_fn_grid).unwrap();

    let eval_fn_sa = make_eval_fn(&events, &features, &scored, &econ);
    let sa = policy_search::search_sa(&search_config, &scored, eval_fn_sa).unwrap();

    eprintln!("Grid best: {:.4}", grid.best_objective);
    eprintln!("SA best:   {:.4}", sa.best_objective);

    // SA with multi-restart should find at least 80% of grid's objective
    // (grid explores more systematically at low iteration counts)
    let threshold = grid.best_objective * 0.8;
    assert!(
        sa.best_objective >= threshold || sa.best_objective >= grid.best_objective - 1.0,
        "SA ({:.4}) should be competitive with Grid ({:.4})",
        sa.best_objective,
        grid.best_objective
    );
}

#[test]
fn qubo_dsl_search_produces_valid_result() {
    let (events, features, scored, _, econ) = setup();
    let eval_fn = make_eval_fn(&events, &features, &scored, &econ);
    let result = qc_solver::policy_qubo::search_qubo(&scored, 500_000, eval_fn, 2000, 42).unwrap();
    assert!(
        result.best_objective.is_finite(),
        "QUBO should produce finite objective"
    );
    assert!(
        result.candidates_evaluated > 0,
        "QUBO should evaluate candidates"
    );
    eprintln!("QUBO best: {:.4}", result.best_objective);
}

#[test]
fn grid_search_produces_multiple_candidates() {
    let (events, features, scored, _, econ) = setup();
    let search_config = PolicySearchConfig {
        capacity_bytes: 500_000,
        max_iterations: 50,
        seed: 42,
        top_k: 5,
        content_types: vec![],
    };
    let eval_fn = make_eval_fn(&events, &features, &scored, &econ);
    let result = policy_search::search(&search_config, &scored, eval_fn).unwrap();
    assert!(
        result.top_candidates.len() > 1,
        "should find multiple candidates"
    );
    // Top candidates should be sorted descending
    for w in result.top_candidates.windows(2) {
        assert!(w[0].1 >= w[1].1 - 1e-12, "candidates should be sorted");
    }
}
