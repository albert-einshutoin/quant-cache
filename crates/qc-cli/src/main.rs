use clap::{Parser, Subcommand};

mod commands;
mod providers;

use commands::{calibrate, compare, generate, import, optimize, policy_eval, simulate};

#[derive(Parser)]
#[command(name = "qc")]
#[command(about = "quant-cache: Economic cache control plane")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Import CDN provider logs into canonical trace format
    Import(import::ImportArgs),
    /// Generate synthetic trace data
    Generate(generate::GenerateArgs),
    /// Optimize cache policy from trace data
    Optimize(optimize::OptimizeArgs),
    /// Evaluate PolicyIR configurations against a trace
    PolicyEval(policy_eval::PolicyEvalArgs),
    /// Replay trace and measure metrics
    Simulate(simulate::SimulateArgs),
    /// Compare baseline policies side-by-side
    Compare(compare::CompareArgs),
    /// Calibrate economic parameters using train/validation traces
    Calibrate(calibrate::CalibrateArgs),
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Import(args) => import::run(args),
        Commands::Generate(args) => generate::run(args),
        Commands::Optimize(args) => optimize::run(args),
        Commands::PolicyEval(args) => policy_eval::run(args),
        Commands::Simulate(args) => simulate::run(args),
        Commands::Compare(args) => compare::run(args),
        Commands::Calibrate(args) => calibrate::run(args),
    }
}
