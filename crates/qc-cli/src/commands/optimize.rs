use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::preset::Preset;
use qc_model::scenario::ScenarioConfig;
use qc_simulate::synthetic;
use qc_solver::score::BenefitCalculator;
use qc_solver::solver::Solver;

#[derive(Args)]
pub struct OptimizeArgs {
    /// Input trace CSV file
    #[arg(short, long)]
    pub input: PathBuf,

    /// Output policy JSON file
    #[arg(short, long, default_value = "policy.json")]
    pub output: PathBuf,

    /// Cache capacity in bytes
    #[arg(long, default_value_t = 10_737_418_240)]
    pub capacity: u64,

    /// Time window in seconds
    #[arg(long, default_value_t = 86400)]
    pub time_window: u64,

    /// Preset profile: ecommerce, media, api
    #[arg(long)]
    pub preset: Option<String>,

    /// TOML config file (overrides preset and CLI flags)
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Use ILP solver instead of greedy
    #[arg(long, default_value_t = false)]
    pub ilp: bool,
}

pub fn run(args: &OptimizeArgs) -> anyhow::Result<()> {
    let events = read_trace_csv(&args.input)?;
    tracing::info!(events = events.len(), "loaded trace");

    let features = synthetic::aggregate_features(&events, args.time_window);
    tracing::info!(objects = features.len(), "aggregated object features");

    let config = load_config(args)?;

    let scored = BenefitCalculator::score_all(&features, &config)?;

    let constraint = qc_model::scenario::CapacityConstraint {
        capacity_bytes: config.capacity_bytes,
    };

    let result = if args.ilp {
        tracing::info!("using ILP solver");
        qc_solver::ilp::ExactIlpSolver.solve(&scored, &constraint)?
    } else {
        tracing::info!("using greedy solver");
        qc_solver::greedy::GreedySolver.solve(&scored, &constraint)?
    };

    // Gather stats before moving decisions
    let num_cached = result.decisions.iter().filter(|d| d.cache).count();
    let num_total = result.decisions.len();
    let objective_value = result.objective_value;
    let solve_time_ms = result.solve_time_ms;
    let shadow_price = result.shadow_price;

    let scored_size_map: std::collections::HashMap<&str, u64> = scored
        .iter()
        .map(|s| (s.cache_key.as_str(), s.size_bytes))
        .collect();
    let cached_bytes: u64 = result
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

    let policy_file = qc_model::policy::PolicyFile {
        solver: qc_model::policy::SolverMetadata {
            solver_name: if args.ilp {
                "ExactILP".into()
            } else {
                "Greedy".into()
            },
            objective_value,
            solve_time_ms,
            shadow_price,
            optimality_gap: result.gap,
            capacity_bytes: config.capacity_bytes,
            cached_bytes,
        },
        decisions: result.decisions,
    };
    let json = serde_json::to_string_pretty(&policy_file)?;
    std::fs::write(&args.output, &json)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Optimized: {num_cached}/{num_total} objects cached")?;
    writeln!(out, "Objective value: {objective_value:.4}")?;
    writeln!(out, "Solve time: {solve_time_ms}ms")?;
    if let Some(sp) = shadow_price {
        writeln!(out, "Shadow price: {sp:.6} $/byte")?;
    }
    writeln!(out, "Policy → {}", args.output.display())?;

    Ok(())
}

pub(crate) fn load_config(args: &OptimizeArgs) -> anyhow::Result<ScenarioConfig> {
    if let Some(config_path) = &args.config {
        let toml_str = std::fs::read_to_string(config_path)?;
        let config: ScenarioConfig = toml::from_str(&toml_str)?;
        return Ok(config);
    }

    Ok(match &args.preset {
        Some(p) => match p.as_str() {
            "ecommerce" => Preset::Ecommerce.to_config(args.capacity),
            "media" => Preset::Media.to_config(args.capacity),
            "api" => Preset::Api.to_config(args.capacity),
            other => anyhow::bail!("unknown preset: {other}. Use: ecommerce, media, api"),
        },
        None => Preset::Ecommerce.to_config(args.capacity),
    })
}

pub(crate) fn read_trace_csv(
    path: &std::path::Path,
) -> anyhow::Result<Vec<qc_model::trace::RequestTraceEvent>> {
    let mut rdr = csv::ReaderBuilder::new().flexible(true).from_path(path)?;
    let mut events = Vec::new();
    for result in rdr.deserialize() {
        let event: qc_model::trace::RequestTraceEvent = result?;
        events.push(event);
    }
    Ok(events)
}
