mod akamai;
mod cloudflare;
mod cloudfront;
mod fastly;

use akamai::{compile_akamai, validate_akamai};
use cloudflare::{compile_cloudflare, validate_cloudflare};
use cloudfront::{compile_cloudfront, validate_cloudfront};
use fastly::{compile_fastly, validate_fastly};

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::policy::PolicyFile;
use qc_model::policy_ir::{AdmissionRule, PolicyIR};

#[derive(Args)]
pub struct CompileArgs {
    /// PolicyIR JSON file
    #[arg(short, long)]
    pub policy: PathBuf,

    /// Target platform: cloudflare, cloudfront, fastly, akamai
    #[arg(short, long, default_value = "cloudflare")]
    pub target: String,

    /// Scores file (PolicyFile JSON from `qc optimize`) for admission gate
    #[arg(long)]
    pub scores: Option<PathBuf>,

    /// Output file
    #[arg(short, long, default_value = "cache-config.json")]
    pub output: PathBuf,

    /// Validate the compiled output against provider schema constraints
    #[arg(long, default_value_t = false)]
    pub validate: bool,
}

pub fn run(args: &CompileArgs) -> anyhow::Result<()> {
    let ir_str = std::fs::read_to_string(&args.policy)?;
    let ir: PolicyIR = serde_json::from_str(&ir_str)?;

    let score_map = if let Some(ref scores_path) = args.scores {
        let pf_str = std::fs::read_to_string(scores_path)?;
        let pf: PolicyFile = serde_json::from_str(&pf_str)?;

        // Normalize score keys through cache_key_rules to match runtime lookups
        let key_regexes: Vec<(regex::Regex, String)> = ir
            .cache_key_rules
            .iter()
            .filter_map(|r| {
                regex::Regex::new(&r.pattern)
                    .ok()
                    .map(|re| (re, r.replacement.clone()))
            })
            .collect();

        let normalize = |key: &str| -> String {
            let mut k = key.to_string();
            for (re, repl) in &key_regexes {
                k = re.replace_all(&k, repl.as_str()).to_string();
            }
            // Sort query parameters alphabetically to match runtime normalization
            if let Some(idx) = k.find('?') {
                let (path, query) = k.split_at(idx + 1);
                let mut params: Vec<&str> = query.split('&').filter(|s| !s.is_empty()).collect();
                params.sort();
                k = format!("{}{}", path, params.join("&"));
                // Remove trailing '?' if all params were stripped
                if k.ends_with('?') {
                    k.pop();
                }
            }
            k
        };

        let use_density = matches!(
            ir.admission_rule,
            AdmissionRule::ScoreDensityThreshold { .. }
        );

        let mut map: HashMap<String, f64> = HashMap::new();
        for d in &pf.decisions {
            let nk = normalize(&d.cache_key);
            let value = if use_density && d.size_bytes > 0 {
                d.score / d.size_bytes as f64
            } else {
                d.score
            };
            let entry = map.entry(nk).or_insert(0.0);
            if value > *entry {
                *entry = value;
            }
        }
        Some(map)
    } else {
        None
    };

    match args.target.as_str() {
        "cloudflare" => compile_cloudflare(&ir, score_map.as_ref(), &args.output),
        "cloudfront" => compile_cloudfront(&ir, score_map.as_ref(), &args.output),
        "fastly" => compile_fastly(&ir, score_map.as_ref(), &args.output),
        "akamai" => compile_akamai(&ir, score_map.as_ref(), &args.output),
        other => {
            anyhow::bail!(
                "unsupported target: {other}. Supported: cloudflare, cloudfront, fastly, akamai"
            )
        }
    }?;

    if args.validate {
        let output_str = std::fs::read_to_string(&args.output)?;
        let config: serde_json::Value = serde_json::from_str(&output_str)?;
        let issues = match args.target.as_str() {
            "cloudflare" => validate_cloudflare(&config),
            "cloudfront" => validate_cloudfront(&config),
            "fastly" => validate_fastly(&config),
            "akamai" => validate_akamai(&config),
            _ => vec![],
        };

        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        if issues.is_empty() {
            writeln!(out, "\nValidation: PASS (0 issues)")?;
        } else {
            writeln!(out, "\nValidation: {} issue(s) found:", issues.len())?;
            for (i, issue) in issues.iter().enumerate() {
                writeln!(out, "  {}: {}", i + 1, issue)?;
            }
            anyhow::bail!("validation failed with {} issue(s)", issues.len());
        }
    }

    Ok(())
}
