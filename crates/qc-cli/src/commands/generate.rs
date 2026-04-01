use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_simulate::synthetic::{self, SyntheticConfig};

#[derive(Args)]
pub struct GenerateArgs {
    /// Output CSV file path
    #[arg(short, long, default_value = "trace.csv")]
    pub output: PathBuf,

    /// Number of unique objects
    #[arg(long, default_value_t = 10_000)]
    pub num_objects: usize,

    /// Number of requests to generate
    #[arg(long, default_value_t = 1_000_000)]
    pub num_requests: usize,

    /// Zipf alpha parameter for popularity distribution
    #[arg(long, default_value_t = 0.8)]
    pub zipf_alpha: f64,

    /// Time window in seconds
    #[arg(long, default_value_t = 86400)]
    pub time_window: u64,

    /// Random seed for reproducibility
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
}

pub fn run(args: &GenerateArgs) -> anyhow::Result<()> {
    let config = SyntheticConfig {
        num_objects: args.num_objects,
        num_requests: args.num_requests,
        zipf_alpha: args.zipf_alpha,
        time_window_seconds: args.time_window,
        seed: args.seed,
        ..SyntheticConfig::default()
    };

    tracing::info!(
        num_objects = config.num_objects,
        num_requests = config.num_requests,
        "generating synthetic trace"
    );

    let events = synthetic::generate(&config)?;

    let mut wtr = csv::Writer::from_writer(std::io::BufWriter::new(std::fs::File::create(
        &args.output,
    )?));

    for event in &events {
        wtr.serialize(event)?;
    }
    wtr.flush()?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(
        out,
        "Generated {} events → {}",
        events.len(),
        args.output.display()
    )?;

    Ok(())
}
