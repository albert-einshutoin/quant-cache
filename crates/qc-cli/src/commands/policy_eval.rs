use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::policy_ir::PolicyIR;
use qc_model::scenario::FreshnessModel;
use qc_simulate::engine::{CachePolicy, ReplayEconConfig, TraceReplayEngine};
use qc_simulate::ir_policy::{IrEvalContext, IrPolicy};
use qc_simulate::synthetic;
use qc_solver::score::BenefitCalculator;

#[derive(Args)]
pub struct PolicyEvalArgs {
    /// Input trace CSV file
    #[arg(short, long)]
    pub input: PathBuf,

    /// PolicyIR JSON file(s). Multiple for comparison.
    #[arg(short, long, num_args = 1..)]
    pub policy: Vec<PathBuf>,

    /// Time window in seconds
    #[arg(long, default_value_t = 86400)]
    pub time_window: u64,

    /// Preset profile: ecommerce, media, api
    #[arg(long)]
    pub preset: Option<String>,

    /// TOML scenario config
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Output metrics JSON
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

pub fn run(args: &PolicyEvalArgs) -> anyhow::Result<()> {
    let events = super::optimize::read_trace_csv(&args.input)?;
    tracing::info!(events = events.len(), "loaded trace");

    // Load scenario config for scoring (reuse optimize's loader)
    let opt_args = super::optimize::OptimizeArgs {
        input: args.input.clone(),
        output: PathBuf::new(),
        capacity: 0, // unused — capacity comes from PolicyIR
        time_window: args.time_window,
        preset: args.preset.clone(),
        config: args.config.clone(),
        solver: "greedy".into(),
        co_access_window_ms: 0,
        co_access_top_k: 0,
        ilp: false,
        scoring: None,
    };
    let scenario_config = super::optimize::load_config(&opt_args)?;

    let compute_reuse =
        scenario_config.scoring_version == qc_model::scenario::ScoringVersion::V2ReuseDistance;
    let features =
        synthetic::aggregate_features_with_options(&events, args.time_window, compute_reuse);

    // Score objects
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

    // Build eval context
    let mut results = Vec::new();
    for policy_path in &args.policy {
        let ir_str = std::fs::read_to_string(policy_path)?;
        let ir: PolicyIR = if policy_path.extension().is_some_and(|e| e == "toml") {
            toml::from_str(&ir_str)?
        } else {
            serde_json::from_str(&ir_str)?
        };

        let eval_ctx = IrEvalContext::from_features_and_scores(&features, &scored);
        let mut policy = IrPolicy::new(ir, eval_ctx);
        let trace_start = events
            .first()
            .map(|e| e.timestamp)
            .unwrap_or_else(chrono::Utc::now);
        policy.prewarm(&features, trace_start);
        policy.apply_ttl_rules(&events);

        let metrics = TraceReplayEngine::replay_with_econ(&events, &mut policy, &econ)?;
        results.push((policy.name().to_string(), metrics));
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    if results.len() == 1 {
        // Single policy: detailed output
        let (name, metrics) = &results[0];
        writeln!(out, "Policy: {name}")?;
        writeln!(out, "  Hit ratio:        {:.2}%", metrics.hit_ratio * 100.0)?;
        writeln!(
            out,
            "  Byte hit ratio:   {:.2}%",
            metrics.byte_hit_ratio * 100.0
        )?;
        writeln!(
            out,
            "  Cost savings:     ${:.4}",
            metrics.estimated_cost_savings
        )?;
        writeln!(
            out,
            "  Objective value:  ${:.4}",
            metrics.policy_objective_value
        )?;
        writeln!(
            out,
            "  Stale serve rate: {:.4}%",
            metrics.stale_serve_rate * 100.0
        )?;
    } else {
        // Multiple policies: comparison table
        writeln!(
            out,
            "{:<30} {:>10} {:>12} {:>14} {:>14}",
            "Policy", "Hit%", "ByteHit%", "CostSavings$", "Objective$"
        )?;
        writeln!(out, "{}", "-".repeat(85))?;
        for (name, metrics) in &results {
            writeln!(
                out,
                "{:<30} {:>9.2}% {:>11.2}% {:>14.4} {:>14.4}",
                name,
                metrics.hit_ratio * 100.0,
                metrics.byte_hit_ratio * 100.0,
                metrics.estimated_cost_savings,
                metrics.policy_objective_value,
            )?;
        }
    }

    if let Some(output) = &args.output {
        let json = serde_json::to_string_pretty(
            &results
                .iter()
                .map(|(n, m)| serde_json::json!({ "name": n, "metrics": m }))
                .collect::<Vec<_>>(),
        )?;
        std::fs::write(output, &json)?;
        writeln!(out, "\nMetrics → {}", output.display())?;
    }

    Ok(())
}
