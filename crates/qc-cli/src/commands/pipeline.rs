use std::io::Write;
use std::path::PathBuf;

use clap::Args;

/// Run the full optimization pipeline: import → optimize → compile → deploy-check.
///
/// Designed for cron/Lambda periodic re-optimization. Skips compile if the
/// policy output is unchanged from the previous run (diff-aware).
#[derive(Args)]
pub struct PipelineArgs {
    /// Input log file (CDN provider logs)
    #[arg(short, long)]
    pub input: PathBuf,

    /// CDN provider: cloudfront, cloudflare, fastly
    #[arg(long, default_value = "cloudfront")]
    pub provider: String,

    /// Working directory for intermediate files (trace.csv, policy.json, etc.)
    #[arg(long, default_value = ".qc-pipeline")]
    pub work_dir: PathBuf,

    /// Cache capacity in bytes [default: 10GB]
    #[arg(long, default_value_t = 10_737_418_240)]
    pub capacity: u64,

    /// Time window in seconds [default: 1 day]
    #[arg(long, default_value_t = 86400)]
    pub time_window: u64,

    /// Preset profile: ecommerce, media, api
    #[arg(long)]
    pub preset: Option<String>,

    /// Compile target: cloudflare, cloudfront, fastly, akamai
    #[arg(long, default_value = "cloudflare")]
    pub target: String,

    /// Policy search method: grid, sa, qubo
    #[arg(long, default_value = "sa")]
    pub search_method: String,

    /// Skip compile if policy is unchanged from previous run
    #[arg(long, default_value_t = true)]
    pub diff_aware: bool,

    /// Max iterations for policy search
    #[arg(long, default_value_t = 100)]
    pub max_iterations: usize,
}

pub fn run(args: &PipelineArgs) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // Create working directory
    std::fs::create_dir_all(&args.work_dir)?;

    let trace_path = args.work_dir.join("trace.csv");
    let policy_path = args.work_dir.join("policy.json");
    let ir_path = args.work_dir.join("policy_ir.json");
    let config_path = args.work_dir.join("compiled-config.json");
    let prev_policy_path = args.work_dir.join("policy.prev.json");

    writeln!(out, "=== quant-cache pipeline ===")?;
    writeln!(out)?;

    // Step 1: Import
    writeln!(out, "[1/5] Importing {} logs...", args.provider)?;
    let import_args = super::import::ImportArgs {
        provider: args.provider.clone(),
        input: args.input.clone(),
        output: trace_path.clone(),
        cost_config: None,
    };
    super::import::run(&import_args)?;

    // Step 2: Optimize
    writeln!(out, "[2/5] Optimizing cache policy...")?;
    let optimize_args = super::optimize::OptimizeArgs {
        input: trace_path.clone(),
        output: policy_path.clone(),
        capacity: args.capacity,
        time_window: args.time_window,
        preset: args.preset.clone(),
        config: None,
        solver: "greedy".into(),
        co_access_window_ms: 0,
        co_access_top_k: 0,
        ilp: false,
        scoring: None,
        purge_group_weight: 0.0,
        origin_group_weight: 0.0,
        group_top_k: 50,
    };
    super::optimize::run(&optimize_args)?;

    // Step 3: Policy search
    writeln!(
        out,
        "[3/5] Searching policy space (method: {})...",
        args.search_method
    )?;
    let search_args = super::policy_search::PolicySearchArgs {
        input: trace_path.clone(),
        capacity: args.capacity,
        time_window: args.time_window,
        preset: args.preset.clone(),
        max_iterations: args.max_iterations,
        top_k: 1,
        method: args.search_method.clone(),
        output: Some(ir_path.clone()),
    };
    super::policy_search::run(&search_args)?;

    // Step 4: Diff check + Compile
    let should_compile = if args.diff_aware && prev_policy_path.exists() && policy_path.exists() {
        let prev = std::fs::read_to_string(&prev_policy_path).unwrap_or_default();
        let curr = std::fs::read_to_string(&policy_path)?;
        prev != curr
    } else {
        true
    };

    if should_compile {
        writeln!(out, "[4/5] Compiling to {} config...", args.target)?;
        let compile_args = super::compile::CompileArgs {
            policy: ir_path.clone(),
            target: args.target.clone(),
            scores: Some(policy_path.clone()),
            output: config_path.clone(),
            validate: true,
        };
        super::compile::run(&compile_args)?;

        // Save current policy as previous for next diff
        std::fs::copy(&policy_path, &prev_policy_path)?;
    } else {
        writeln!(out, "[4/5] Policy unchanged — skipping compile")?;
    }

    // Step 5: Deploy check
    writeln!(out, "[5/5] Running deploy safety check...")?;
    let deploy_args = super::deploy_check::DeployCheckArgs {
        input: trace_path.clone(),
        policy: ir_path,
        time_window: args.time_window,
        preset: args.preset.clone(),
        min_hit_improvement: -0.05,
        min_objective_improvement: 0.0,
        max_stale_rate: 0.20,
    };
    super::deploy_check::run(&deploy_args)?;

    writeln!(out)?;
    writeln!(out, "=== Pipeline complete ===")?;
    writeln!(out, "  Config: {}", config_path.display())?;
    writeln!(out, "  Policy: {}", policy_path.display())?;
    if !should_compile {
        writeln!(out, "  (compile skipped — no policy change)")?;
    }

    Ok(())
}
