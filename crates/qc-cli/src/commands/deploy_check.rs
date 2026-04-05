use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::policy_ir::PolicyIR;
use qc_model::scenario::FreshnessModel;
use qc_simulate::baselines::{LruPolicy, SievePolicy};
use qc_simulate::engine::{CachePolicy, ReplayEconConfig, TraceReplayEngine};
use qc_simulate::ir_policy::{IrEvalContext, IrPolicy};
use qc_simulate::synthetic;
use qc_solver::score::BenefitCalculator;

#[derive(Args)]
pub struct DeployCheckArgs {
    /// Input trace CSV file
    #[arg(short, long)]
    pub input: PathBuf,

    /// PolicyIR JSON file to validate
    #[arg(short, long)]
    pub policy: PathBuf,

    /// Time window in seconds
    #[arg(long, default_value_t = 86400)]
    pub time_window: u64,

    /// Preset profile: ecommerce, media, api
    #[arg(long)]
    pub preset: Option<String>,

    /// Minimum hit ratio improvement over LRU (default: -0.05 = allow 5% regression)
    #[arg(long, default_value_t = -0.05)]
    pub min_hit_improvement: f64,

    /// Minimum objective improvement over LRU (default: 0.0 = must not regress)
    #[arg(long, default_value_t = 0.0)]
    pub min_objective_improvement: f64,

    /// Maximum stale serve rate (default: 0.20 = 20%)
    #[arg(long, default_value_t = 0.20)]
    pub max_stale_rate: f64,
}

pub fn run(args: &DeployCheckArgs) -> anyhow::Result<()> {
    let events = super::optimize::read_trace_csv(&args.input)?;
    tracing::info!(events = events.len(), "loaded trace");

    // Load scenario config
    let opt_args = super::optimize::OptimizeArgs {
        input: args.input.clone(),
        output: PathBuf::new(),
        capacity: 0,
        time_window: args.time_window,
        preset: args.preset.clone(),
        config: None,
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
    let scored = BenefitCalculator::score_all(&features, &scenario_config)?;

    // Build econ config
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

    // Load PolicyIR
    let ir_str = std::fs::read_to_string(&args.policy)?;
    let ir: PolicyIR = serde_json::from_str(&ir_str)?;

    let trace_start = events
        .first()
        .map(|e| e.timestamp)
        .unwrap_or_else(chrono::Utc::now);

    // 1. Baseline: LRU
    let mut lru = LruPolicy::new(ir.capacity_bytes);
    let lru_metrics = TraceReplayEngine::replay_with_econ(&events, &mut lru, &econ)?;

    // 2. Reference: SIEVE
    let mut sieve = SievePolicy::new(ir.capacity_bytes);
    let sieve_metrics = TraceReplayEngine::replay_with_econ(&events, &mut sieve, &econ)?;

    // 3. Candidate: PolicyIR
    let ctx = IrEvalContext::from_features_and_scores(&features, &scored);
    let mut ir_policy = IrPolicy::new(ir, ctx);
    ir_policy.prewarm(&features, trace_start);
    ir_policy.apply_ttl_rules(&events);
    let ir_metrics = TraceReplayEngine::replay_with_econ(&events, &mut ir_policy, &econ)?;

    // Compare
    let hit_improvement = ir_metrics.hit_ratio - lru_metrics.hit_ratio;
    let obj_improvement = ir_metrics.policy_objective_value - lru_metrics.policy_objective_value;
    let stale_rate = ir_metrics.stale_serve_rate;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    writeln!(out, "Deploy Check Results")?;
    writeln!(out, "===================")?;
    writeln!(out)?;
    writeln!(
        out,
        "{:<20} {:>10} {:>14} {:>12}",
        "Policy", "Hit%", "Objective$", "Stale%"
    )?;
    writeln!(out, "{}", "-".repeat(60))?;
    writeln!(
        out,
        "{:<20} {:>9.2}% {:>14.4} {:>11.4}%",
        "LRU (baseline)",
        lru_metrics.hit_ratio * 100.0,
        lru_metrics.policy_objective_value,
        lru_metrics.stale_serve_rate * 100.0,
    )?;
    writeln!(
        out,
        "{:<20} {:>9.2}% {:>14.4} {:>11.4}%",
        "SIEVE (reference)",
        sieve_metrics.hit_ratio * 100.0,
        sieve_metrics.policy_objective_value,
        sieve_metrics.stale_serve_rate * 100.0,
    )?;
    writeln!(
        out,
        "{:<20} {:>9.2}% {:>14.4} {:>11.4}%",
        ir_policy.name(),
        ir_metrics.hit_ratio * 100.0,
        ir_metrics.policy_objective_value,
        ir_metrics.stale_serve_rate * 100.0,
    )?;

    writeln!(out)?;
    writeln!(out, "Safety Checks")?;
    writeln!(out, "-------------")?;

    let mut passed = true;

    // Check 1: Hit ratio vs LRU
    let hit_ok = hit_improvement >= args.min_hit_improvement;
    writeln!(
        out,
        "  [{}] Hit improvement vs LRU: {:+.2}% (min: {:+.2}%)",
        if hit_ok { "PASS" } else { "FAIL" },
        hit_improvement * 100.0,
        args.min_hit_improvement * 100.0,
    )?;
    if !hit_ok {
        passed = false;
    }

    // Check 2: Objective vs LRU
    let obj_ok = obj_improvement >= args.min_objective_improvement;
    writeln!(
        out,
        "  [{}] Objective improvement vs LRU: {:+.4}$ (min: {:+.4}$)",
        if obj_ok { "PASS" } else { "FAIL" },
        obj_improvement,
        args.min_objective_improvement,
    )?;
    if !obj_ok {
        passed = false;
    }

    // Check 3: Stale rate
    let stale_ok = stale_rate <= args.max_stale_rate;
    writeln!(
        out,
        "  [{}] Stale serve rate: {:.4}% (max: {:.2}%)",
        if stale_ok { "PASS" } else { "FAIL" },
        stale_rate * 100.0,
        args.max_stale_rate * 100.0,
    )?;
    if !stale_ok {
        passed = false;
    }

    // Check 4: Not worse than SIEVE on objective
    let sieve_ok = ir_metrics.policy_objective_value >= sieve_metrics.policy_objective_value * 0.8;
    writeln!(
        out,
        "  [{}] Objective ≥ 80% of SIEVE: {:.4}$ vs {:.4}$",
        if sieve_ok { "PASS" } else { "WARN" },
        ir_metrics.policy_objective_value,
        sieve_metrics.policy_objective_value,
    )?;

    writeln!(out)?;
    if passed {
        writeln!(out, "Result: DEPLOY SAFE")?;
    } else {
        writeln!(out, "Result: DEPLOY BLOCKED")?;
        anyhow::bail!("deploy check failed — policy does not meet safety thresholds");
    }

    Ok(())
}
