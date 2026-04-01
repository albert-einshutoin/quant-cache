use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::origin_cost::OriginCostConfig;

use crate::providers::cloudfront::CloudFrontParser;
use crate::providers::ProviderLogParser;

#[derive(Args)]
pub struct ImportArgs {
    /// CDN provider: cloudfront
    #[arg(short, long)]
    pub provider: String,

    /// Input log file path
    #[arg(short, long)]
    pub input: PathBuf,

    /// Output canonical trace CSV file
    #[arg(short, long, default_value = "trace.csv")]
    pub output: PathBuf,

    /// Origin cost config TOML file (optional)
    #[arg(long)]
    pub cost_config: Option<PathBuf>,
}

pub fn run(args: &ImportArgs) -> anyhow::Result<()> {
    let cost_config = if let Some(ref path) = args.cost_config {
        let toml_str = std::fs::read_to_string(path)?;
        toml::from_str(&toml_str)?
    } else {
        OriginCostConfig::default()
    };

    let parser: Box<dyn ProviderLogParser> = match args.provider.as_str() {
        "cloudfront" => Box::new(CloudFrontParser),
        other => anyhow::bail!("unsupported provider: {other}. Supported: cloudfront"),
    };

    tracing::info!(provider = parser.name(), "importing logs");

    let events = parser.parse(&args.input, &cost_config)?;

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
        "Imported {} events from {} → {}",
        events.len(),
        args.input.display(),
        args.output.display()
    )?;

    Ok(())
}
