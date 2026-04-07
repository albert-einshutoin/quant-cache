use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::compact_trace::CompactTraceEvent;
use qc_model::policy::{PolicyDecision, PolicyFile};
use qc_model::scenario::StalePenaltyClass;
use qc_simulate::compact_baselines::CompactStaticPolicy;
use qc_simulate::engine::{CompactReplayEconConfig, ReplayEconConfig, TraceReplayEngine};
use qc_simulate::synthetic;

#[derive(Args)]
pub struct SimulateArgs {
    /// Input trace CSV file
    #[arg(short, long)]
    pub input: PathBuf,

    /// Policy JSON file (output of `qc optimize`)
    #[arg(short, long)]
    pub policy: PathBuf,

    /// Cache capacity in bytes (for capacity_utilization calculation)
    #[arg(long, default_value_t = 10_737_418_240)]
    pub capacity: u64,

    /// Time window in seconds
    #[arg(long, default_value_t = 86400)]
    pub time_window: u64,

    /// Latency value ($/ms) for economic objective calculation
    #[arg(long, default_value_t = 0.00005)]
    pub latency_value: f64,

    /// Default stale penalty class (none, low, medium, high, very_high)
    #[arg(long, default_value = "high")]
    pub stale_penalty_class: String,

    /// Output metrics JSON file
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

pub fn run(args: &SimulateArgs) -> anyhow::Result<()> {
    let events = super::optimize::read_trace_csv(&args.input)?;
    tracing::info!(events = events.len(), "loaded trace");

    // Load policy file (new format with solver metadata, or legacy array)
    let policy_json = std::fs::read_to_string(&args.policy)?;
    let (decisions, solver_meta) = match serde_json::from_str::<PolicyFile>(&policy_json) {
        Ok(pf) => (pf.decisions, Some(pf.solver)),
        Err(_) => {
            // Fallback: legacy format (bare array of decisions)
            let decisions: Vec<PolicyDecision> = serde_json::from_str(&policy_json)?;
            (decisions, None)
        }
    };

    // Build per-object econ from trace features (needs original events for aggregation)
    let features = synthetic::aggregate_features(&events, args.time_window);
    let default_class = parse_stale_class(&args.stale_penalty_class)?;
    let econ = ReplayEconConfig::from_features(&features, args.latency_value, default_class);

    // Convert to compact representation for memory-efficient replay
    let (compact_events, mut interner) = CompactTraceEvent::intern_batch(&events);
    drop(events); // free original String-heavy events

    let compact_econ = CompactReplayEconConfig::from_econ_config(&econ, &mut interner);

    let cached_key_ids: Vec<u32> = decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| interner.intern(&d.cache_key))
        .collect();

    let mut policy = CompactStaticPolicy::new(cached_key_ids);
    let mut metrics =
        TraceReplayEngine::replay_compact_with_econ(&compact_events, &mut policy, &compact_econ)?;

    // Fill solver diagnostics from policy metadata
    if let Some(ref meta) = solver_meta {
        metrics.solve_time_ms = meta.solve_time_ms;
        metrics.optimality_gap = meta.optimality_gap;
        metrics.capacity_utilization = if meta.capacity_bytes > 0 {
            meta.cached_bytes as f64 / meta.capacity_bytes as f64
        } else {
            0.0
        };
    } else {
        // Legacy fallback: compute from features
        let feature_size_map: std::collections::HashMap<&str, u64> = features
            .iter()
            .map(|f| (f.cache_key.as_str(), f.size_bytes))
            .collect();
        let actual_cached_bytes: u64 = decisions
            .iter()
            .filter(|d| d.cache)
            .map(|d| {
                feature_size_map
                    .get(d.cache_key.as_str())
                    .copied()
                    .unwrap_or(0)
            })
            .sum();
        metrics.capacity_utilization = if args.capacity > 0 {
            actual_cached_bytes as f64 / args.capacity as f64
        } else {
            0.0
        };
    }

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Simulation Results")?;
    writeln!(
        out,
        "  Hit ratio:          {:.2}%",
        metrics.hit_ratio * 100.0
    )?;
    writeln!(
        out,
        "  Byte hit ratio:     {:.2}%",
        metrics.byte_hit_ratio * 100.0
    )?;
    writeln!(
        out,
        "  Cost savings:       ${:.4}",
        metrics.estimated_cost_savings
    )?;
    writeln!(
        out,
        "  Objective value:    ${:.4}",
        metrics.policy_objective_value
    )?;
    writeln!(
        out,
        "  Origin egress:      {} bytes",
        metrics.origin_egress_bytes
    )?;
    writeln!(
        out,
        "  Stale serve rate:   {:.4}%",
        metrics.stale_serve_rate * 100.0
    )?;
    writeln!(out, "  Stale serve count:  {}", metrics.stale_serve_count)?;
    writeln!(
        out,
        "  Capacity util:      {:.1}%",
        metrics.capacity_utilization * 100.0
    )?;
    writeln!(out, "  Solve time:         {}ms", metrics.solve_time_ms)?;
    if let Some(gap) = metrics.optimality_gap {
        writeln!(out, "  Optimality gap:     {:.2}%", gap * 100.0)?;
    }

    if let Some(output) = &args.output {
        let json = serde_json::to_string_pretty(&metrics)?;
        std::fs::write(output, &json)?;
        writeln!(out, "  Metrics → {}", output.display())?;
    }

    Ok(())
}

fn parse_stale_class(s: &str) -> anyhow::Result<StalePenaltyClass> {
    match s {
        "none" => Ok(StalePenaltyClass::None),
        "low" => Ok(StalePenaltyClass::Low),
        "medium" => Ok(StalePenaltyClass::Medium),
        "high" => Ok(StalePenaltyClass::High),
        "very_high" => Ok(StalePenaltyClass::VeryHigh),
        other => anyhow::bail!(
            "unknown stale penalty class: {other}. Use: none, low, medium, high, very_high"
        ),
    }
}
