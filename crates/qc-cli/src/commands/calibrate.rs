use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_simulate::synthetic;
use qc_solver::calibrate;

#[derive(Args)]
pub struct CalibrateArgs {
    /// Training trace CSV file
    #[arg(long)]
    pub train: PathBuf,

    /// Validation trace CSV file
    #[arg(long)]
    pub validation: PathBuf,

    /// Cache capacity in bytes
    #[arg(long, default_value_t = 10_737_418_240)]
    pub capacity: u64,

    /// Time window in seconds
    #[arg(long, default_value_t = 86400)]
    pub time_window: u64,

    /// Number of random restarts
    #[arg(long, default_value_t = 3)]
    pub restarts: usize,

    /// Output calibrated config TOML file
    #[arg(short, long, default_value = "calibrated.toml")]
    pub output: PathBuf,
}

pub fn run(args: &CalibrateArgs) -> anyhow::Result<()> {
    let train_events = super::optimize::read_trace_csv(&args.train)?;
    let val_events = super::optimize::read_trace_csv(&args.validation)?;

    tracing::info!(
        train = train_events.len(),
        val = val_events.len(),
        "loaded traces"
    );

    let train_features = synthetic::aggregate_features(&train_events, args.time_window);
    let val_features = synthetic::aggregate_features(&val_events, args.time_window);

    let base_config = qc_model::preset::Preset::Ecommerce.to_config(args.capacity);

    let result = calibrate::calibrate(
        &train_features,
        &train_events,
        &val_features,
        &val_events,
        &base_config,
        args.restarts,
    )?;

    // Write calibrated config
    let toml_str = toml::to_string_pretty(&result.best_config)?;
    std::fs::write(&args.output, &toml_str)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Calibration Results")?;
    writeln!(out, "  Best validation score: ${:.4}", result.best_score)?;
    writeln!(out, "  Iterations: {}", result.iterations)?;
    writeln!(
        out,
        "  latency_value_per_ms: {}",
        result.best_config.latency_value_per_ms
    )?;

    if let qc_model::scenario::FreshnessModel::TtlOnly { ref stale_penalty } =
        result.best_config.freshness_model
    {
        writeln!(
            out,
            "  stale_penalty_class: {:?}",
            stale_penalty.default_class
        )?;
    }

    writeln!(out, "\nSensitivity (latency_value_per_ms):")?;
    for (name, value, delta) in &result.parameter_sensitivity {
        writeln!(out, "  {name}={value:.6} → delta=${delta:+.4}")?;
    }

    writeln!(out, "\nConfig → {}", args.output.display())?;

    Ok(())
}
