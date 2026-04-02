use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::policy_ir::PolicyIR;
use qc_model::scenario::FreshnessModel;
use qc_simulate::engine::{CachePolicy, ReplayEconConfig, TraceReplayEngine};
use qc_simulate::ir_policy::{IrEvalContext, IrPolicy};
use qc_simulate::synthetic;
use qc_solver::policy_search::{self, PolicySearchConfig};
use qc_solver::score::BenefitCalculator;

#[derive(Args)]
pub struct PolicySearchArgs {
    /// Input trace CSV file
    #[arg(short, long)]
    pub input: PathBuf,

    /// Cache capacity in bytes
    #[arg(long, default_value_t = 10_737_418_240)]
    pub capacity: u64,

    /// Time window in seconds
    #[arg(long, default_value_t = 86400)]
    pub time_window: u64,

    /// Preset profile: ecommerce, media, api
    #[arg(long)]
    pub preset: Option<String>,

    /// Max candidates to evaluate
    #[arg(long, default_value_t = 200)]
    pub max_iterations: usize,

    /// Number of top candidates to show
    #[arg(long, default_value_t = 5)]
    pub top_k: usize,

    /// Output best PolicyIR as JSON
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

pub fn run(args: &PolicySearchArgs) -> anyhow::Result<()> {
    let events = super::optimize::read_trace_csv(&args.input)?;
    tracing::info!(events = events.len(), "loaded trace");

    let features = synthetic::aggregate_features(&events, args.time_window);

    let opt_args = super::optimize::OptimizeArgs {
        input: args.input.clone(),
        output: PathBuf::new(),
        capacity: args.capacity,
        time_window: args.time_window,
        preset: args.preset.clone(),
        config: None,
        solver: "greedy".into(),
        co_access_window_ms: 0,
        co_access_top_k: 0,
        ilp: false,
    };
    let scenario_config = super::optimize::load_config(&opt_args)?;
    let scored = BenefitCalculator::score_all(&features, &scenario_config)?;

    // Build econ config for replay
    let default_class = match &scenario_config.freshness_model {
        FreshnessModel::TtlOnly { stale_penalty } => stale_penalty.default_class,
        FreshnessModel::InvalidationOnUpdate { .. } => qc_model::scenario::StalePenaltyClass::None,
    };
    let econ = match &scenario_config.freshness_model {
        FreshnessModel::TtlOnly { stale_penalty } => {
            ReplayEconConfig::from_features_with_overrides(
                &features,
                scenario_config.latency_value_per_ms,
                default_class,
                &stale_penalty.cost_overrides,
            )
        }
        _ => ReplayEconConfig::from_features(
            &features,
            scenario_config.latency_value_per_ms,
            default_class,
        ),
    };

    let trace_start = events
        .first()
        .map(|e| e.timestamp)
        .unwrap_or_else(chrono::Utc::now);

    let search_config = PolicySearchConfig {
        capacity_bytes: args.capacity,
        max_iterations: args.max_iterations,
        seed: 42,
        top_k: args.top_k,
    };

    // Eval function: build IrPolicy from IR, replay, return objective
    let eval_fn = |ir: &PolicyIR| -> Result<f64, qc_solver::error::SolverError> {
        let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
        let mut policy = IrPolicy::new(ir.clone(), ctx);
        policy.prewarm(&features, trace_start);
        policy.apply_ttl_rules(&events);

        let metrics = TraceReplayEngine::replay_with_econ(&events, &mut policy, &econ)
            .map_err(|e| qc_solver::error::SolverError::SolverFailure(e.to_string()))?;

        Ok(metrics.policy_objective_value)
    };

    tracing::info!(
        max_iterations = search_config.max_iterations,
        "starting policy search"
    );
    let result = policy_search::search(&search_config, &scored, eval_fn)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Policy Search Results")?;
    writeln!(
        out,
        "  Candidates evaluated: {}",
        result.candidates_evaluated
    )?;
    writeln!(out, "  Search time: {}ms", result.search_time_ms)?;
    writeln!(out)?;
    writeln!(out, "Top {} candidates:", result.top_candidates.len())?;
    writeln!(out, "{:<5} {:<40} {:>14}", "Rank", "Policy", "Objective$")?;
    writeln!(out, "{}", "-".repeat(62))?;
    for (i, (ir, obj)) in result.top_candidates.iter().enumerate() {
        let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
        let policy = IrPolicy::new(ir.clone(), ctx);
        writeln!(out, "{:<5} {:<40} {:>14.4}", i + 1, policy.name(), obj)?;
    }

    if let Some(output) = &args.output {
        let json = serde_json::to_string_pretty(&result.best_ir)?;
        std::fs::write(output, &json)?;
        writeln!(out, "\nBest policy → {}", output.display())?;
    }

    Ok(())
}
