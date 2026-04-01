use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::scenario::FreshnessModel;
use qc_simulate::baselines::{GdsfPolicy, LruPolicy, StaticPolicy};
use qc_simulate::comparator::Comparator;
use qc_simulate::engine::{CachePolicy, ReplayEconConfig};
use qc_simulate::synthetic;
use qc_solver::score::BenefitCalculator;
use qc_solver::solver::Solver;

#[derive(Args)]
pub struct CompareArgs {
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

    /// TOML config file
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Include ILP solver in comparison (slow for large n)
    #[arg(long, default_value_t = false)]
    pub include_ilp: bool,

    /// Include Belady oracle (optimal eviction, requires full trace pre-index)
    #[arg(long, default_value_t = false)]
    pub include_belady: bool,

    /// Output comparison JSON file
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

pub fn run(args: &CompareArgs) -> anyhow::Result<()> {
    let events = super::optimize::read_trace_csv(&args.input)?;
    tracing::info!(events = events.len(), "loaded trace");

    let opt_args = super::optimize::OptimizeArgs {
        input: args.input.clone(),
        output: PathBuf::new(),
        capacity: args.capacity,
        time_window: args.time_window,
        preset: args.preset.clone(),
        config: args.config.clone(),
        solver: "greedy".into(),
        co_access_window_ms: 0,
        co_access_top_k: 0,
        ilp: false,
    };
    let config = super::optimize::load_config(&opt_args)?;

    let features = synthetic::aggregate_features(&events, args.time_window);
    let scored = BenefitCalculator::score_all(&features, &config)?;
    let constraint = qc_model::scenario::CapacityConstraint {
        capacity_bytes: config.capacity_bytes,
    };

    // Greedy solve
    let greedy_result = qc_solver::greedy::GreedySolver.solve(&scored, &constraint)?;

    // Compute capacity utilization by cache_key join (not positional zip)
    let scored_size_map: std::collections::HashMap<&str, u64> = scored
        .iter()
        .map(|s| (s.cache_key.as_str(), s.size_bytes))
        .collect();
    let greedy_cached_bytes: u64 = greedy_result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| {
            scored_size_map
                .get(d.cache_key.as_str())
                .copied()
                .unwrap_or(0)
        })
        .sum();

    let greedy_keys = greedy_result
        .decisions
        .iter()
        .filter(|d| d.cache)
        .map(|d| d.cache_key.clone());

    // Build per-object econ config matching solver objective
    let default_class = match &config.freshness_model {
        FreshnessModel::TtlOnly { stale_penalty } => stale_penalty.default_class,
        FreshnessModel::InvalidationOnUpdate { .. } => qc_model::scenario::StalePenaltyClass::None,
    };
    let econ = match &config.freshness_model {
        FreshnessModel::TtlOnly { stale_penalty } => {
            ReplayEconConfig::from_features_with_overrides(
                &features,
                config.latency_value_per_ms,
                default_class,
                &stale_penalty.cost_overrides,
            )
        }
        _ => ReplayEconConfig::from_features(&features, config.latency_value_per_ms, default_class),
    };

    let mut lru = LruPolicy::new(config.capacity_bytes);
    let mut gdsf = GdsfPolicy::new(config.capacity_bytes);
    let mut economic = StaticPolicy::new(greedy_keys);

    let mut policies: Vec<&mut dyn CachePolicy> = vec![&mut lru, &mut gdsf, &mut economic];

    // Optional Belady
    let mut belady_policy;
    if args.include_belady {
        belady_policy = Some(qc_simulate::baselines::BeladyPolicy::new(
            &events,
            config.capacity_bytes,
        ));
        policies.push(belady_policy.as_mut().unwrap());
    }

    // Optional ILP
    let ilp_result;
    let mut ilp_policy;
    if args.include_ilp {
        ilp_result = Some(qc_solver::ilp::ExactIlpSolver.solve(&scored, &constraint)?);
        if let Some(ref r) = ilp_result {
            let ilp_keys = r
                .decisions
                .iter()
                .filter(|d| d.cache)
                .map(|d| d.cache_key.clone());
            ilp_policy = Some(StaticPolicy::new_with_name(ilp_keys, "ExactILP"));
            policies.push(ilp_policy.as_mut().unwrap());
        }
    } else {
        ilp_result = None;
    }

    let mut report = Comparator::compare_with_econ(&events, &mut policies, &econ)?;

    // Fill solver-level metrics into the EconomicGreedy result
    for r in &mut report.results {
        if r.name == "EconomicGreedy" {
            r.metrics.solve_time_ms = greedy_result.solve_time_ms;
            r.metrics.capacity_utilization = if config.capacity_bytes > 0 {
                greedy_cached_bytes as f64 / config.capacity_bytes as f64
            } else {
                0.0
            };
        }
    }

    // Fill optimality_gap if ILP included
    if let Some(ref ilp_r) = ilp_result {
        if ilp_r.objective_value > 1e-12 {
            let gap =
                (ilp_r.objective_value - greedy_result.objective_value) / ilp_r.objective_value;
            for r in &mut report.results {
                if r.name == "EconomicGreedy" {
                    r.metrics.optimality_gap = Some(gap);
                }
            }
        }
    }

    // Print table
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(
        out,
        "{:<20} {:>10} {:>12} {:>14} {:>14}",
        "Policy", "Hit%", "ByteHit%", "CostSavings$", "Objective$"
    )?;
    writeln!(out, "{}", "-".repeat(75))?;
    for r in &report.results {
        writeln!(
            out,
            "{:<20} {:>9.2}% {:>11.2}% {:>14.4} {:>14.4}",
            r.name,
            r.metrics.hit_ratio * 100.0,
            r.metrics.byte_hit_ratio * 100.0,
            r.metrics.estimated_cost_savings,
            r.metrics.policy_objective_value,
        )?;
    }

    if let Some(ref ilp_r) = ilp_result {
        if ilp_r.objective_value > 0.0 {
            let gap =
                (ilp_r.objective_value - greedy_result.objective_value) / ilp_r.objective_value;
            writeln!(out)?;
            writeln!(out, "Optimality gap (greedy vs ILP): {:.2}%", gap * 100.0)?;
        }
    }

    writeln!(out)?;
    if let Some(best) = report.best_by_objective() {
        writeln!(out, "Best by objective: {}", best.name)?;
    }
    if let Some(best) = report.best_by_cost_savings() {
        writeln!(out, "Best by cost savings: {}", best.name)?;
    }

    for r in &report.results {
        if r.name == "EconomicGreedy" {
            writeln!(out, "\nEconomicGreedy diagnostics:")?;
            writeln!(out, "  Solve time: {}ms", r.metrics.solve_time_ms)?;
            writeln!(
                out,
                "  Capacity utilization: {:.1}%",
                r.metrics.capacity_utilization * 100.0
            )?;
            writeln!(
                out,
                "  Stale serve rate: {:.4}%",
                r.metrics.stale_serve_rate * 100.0
            )?;
            if let Some(gap) = r.metrics.optimality_gap {
                writeln!(out, "  Optimality gap: {:.2}%", gap * 100.0)?;
            }
        }
    }

    if let Some(output) = &args.output {
        let json = serde_json::to_string_pretty(&report.results)?;
        std::fs::write(output, &json)?;
        writeln!(out, "\nReport → {}", output.display())?;
    }

    Ok(())
}
