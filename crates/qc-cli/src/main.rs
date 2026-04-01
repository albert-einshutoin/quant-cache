use clap::{Parser, Subcommand};

mod commands;
mod providers;

use commands::{compare, generate, import, optimize, simulate};

#[derive(Parser)]
#[command(name = "qc")]
#[command(about = "quant-cache: Economic CDN cache optimization engine")]
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
    /// Replay trace and measure metrics
    Simulate(simulate::SimulateArgs),
    /// Compare policies against baselines (LRU, GDSF, EconomicGreedy)
    Compare(compare::CompareArgs),
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
        Commands::Simulate(args) => simulate::run(args),
        Commands::Compare(args) => compare::run(args),
    }
}
