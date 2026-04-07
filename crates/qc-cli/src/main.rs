use clap::{Parser, Subcommand};

mod commands;
mod providers;

use commands::{
    calibrate, compare, compile, compile_compare, deploy_check, generate, import, optimize,
    pipeline, policy_eval, policy_search, simulate,
};

#[derive(Parser)]
#[command(name = "qc")]
#[command(
    about = "quant-cache: Economic cache decision framework for CDN operators\n\n\
    Evaluate cache policies through economic objectives ($/period),\n\
    search the policy design space, and generate vendor-native configs.\n\n\
    Quick start:\n  \
    qc generate -o trace.csv\n  \
    qc optimize -i trace.csv -o policy.json\n  \
    qc compare -i trace.csv\n  \
    qc compile -p policy_ir.json --target cloudflare"
)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Import CDN provider logs into canonical trace CSV
    ///
    /// Supported providers: cloudfront, cloudflare, fastly
    Import(import::ImportArgs),
    /// Generate synthetic trace data with configurable distributions
    Generate(generate::GenerateArgs),
    /// Optimize cache policy from trace data (greedy/ILP/SA solver)
    Optimize(optimize::OptimizeArgs),
    /// Evaluate a PolicyIR configuration against a trace
    PolicyEval(policy_eval::PolicyEvalArgs),
    /// Search for the best PolicyIR configuration (grid/SA/QUBO)
    PolicySearch(policy_search::PolicySearchArgs),
    /// Replay a trace through EconomicGreedy and measure metrics
    Simulate(simulate::SimulateArgs),
    /// Compare baseline policies (LRU, GDSF, SIEVE, S3-FIFO, Belady)
    Compare(compare::CompareArgs),
    /// Calibrate latency_value and stale_penalty via coordinate descent
    Calibrate(calibrate::CalibrateArgs),
    /// Generate deployment config from PolicyIR for a CDN provider
    ///
    /// Targets: cloudflare, cloudfront, fastly, akamai
    Compile(compile::CompileArgs),
    /// Pre-deploy safety check: replay PolicyIR and verify thresholds
    DeployCheck(deploy_check::DeployCheckArgs),
    /// Compare compiled output across all 4 CDN providers
    CompileCompare(compile_compare::CompileCompareArgs),
    /// Run full pipeline: import → optimize → search → compile → deploy-check
    ///
    /// Designed for cron/Lambda periodic re-optimization. Diff-aware: skips
    /// compile if policy is unchanged from previous run.
    Pipeline(pipeline::PipelineArgs),
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
        Commands::PolicySearch(args) => policy_search::run(args),
        Commands::Simulate(args) => simulate::run(args),
        Commands::Compare(args) => compare::run(args),
        Commands::Calibrate(args) => calibrate::run(args),
        Commands::Compile(args) => compile::run(args),
        Commands::DeployCheck(args) => deploy_check::run(args),
        Commands::CompileCompare(args) => compile_compare::run(args),
        Commands::Pipeline(args) => pipeline::run(args),
    }
}
