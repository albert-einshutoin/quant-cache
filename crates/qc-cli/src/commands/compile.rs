use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::policy::PolicyFile;
use qc_model::policy_ir::{AdmissionRule, Backend, BypassRule, PolicyIR};

#[derive(Args)]
pub struct CompileArgs {
    /// PolicyIR JSON file
    #[arg(short, long)]
    pub policy: PathBuf,

    /// Target platform: cloudflare
    #[arg(short, long, default_value = "cloudflare")]
    pub target: String,

    /// Scores file (PolicyFile JSON from `qc optimize`) for admission gate population
    #[arg(long)]
    pub scores: Option<PathBuf>,

    /// Output file
    #[arg(short, long, default_value = "cache-config.json")]
    pub output: PathBuf,
}

pub fn run(args: &CompileArgs) -> anyhow::Result<()> {
    let ir_str = std::fs::read_to_string(&args.policy)?;
    let ir: PolicyIR = serde_json::from_str(&ir_str)?;

    let score_map = if let Some(ref scores_path) = args.scores {
        let pf_str = std::fs::read_to_string(scores_path)?;
        let pf: PolicyFile = serde_json::from_str(&pf_str)?;
        let map: HashMap<String, f64> = pf
            .decisions
            .iter()
            .map(|d| (d.cache_key.clone(), d.score))
            .collect();
        Some(map)
    } else {
        None
    };

    match args.target.as_str() {
        "cloudflare" => compile_cloudflare(&ir, score_map.as_ref(), &args.output),
        other => anyhow::bail!("unsupported target: {other}. Supported: cloudflare"),
    }
}

fn compile_cloudflare(
    ir: &PolicyIR,
    score_map: Option<&HashMap<String, f64>>,
    output: &std::path::Path,
) -> anyhow::Result<()> {
    let mut rules = Vec::new();

    // 1. Bypass rules
    compile_bypass_rules(&ir.bypass_rule, &mut rules);

    // 2. TTL class rules
    for rule in &ir.ttl_class_rules {
        let expression = if rule.content_type_pattern.ends_with('/') {
            format!(
                "starts_with(http.response.headers[\"content-type\"][0], \"{}\")",
                rule.content_type_pattern
            )
        } else {
            format!(
                "http.response.headers[\"content-type\"][0] eq \"{}\"",
                rule.content_type_pattern
            )
        };

        rules.push(serde_json::json!({
            "description": format!("TTL override: {} → {}s", rule.content_type_pattern, rule.ttl_seconds),
            "expression": expression,
            "action": "set_cache_ttl",
            "action_parameters": { "cache_ttl": rule.ttl_seconds }
        }));
    }

    // 3. Worker script
    let worker_script = match &ir.admission_rule {
        AdmissionRule::Always => None,
        AdmissionRule::ScoreThreshold { threshold } => {
            Some(generate_admission_worker(*threshold, "score", score_map))
        }
        AdmissionRule::ScoreDensityThreshold { threshold } => {
            Some(generate_admission_worker(*threshold, "density", score_map))
        }
    };

    let backend_note = match ir.backend {
        Backend::Sieve => "Cloudflare default caching (closest to SIEVE behavior)",
        Backend::S3Fifo => "Cloudflare default caching (S3-FIFO not directly configurable)",
    };

    let config = serde_json::json!({
        "_generated_by": "quant-cache",
        "_policy_ir": {
            "backend": format!("{:?}", ir.backend),
            "capacity_bytes": ir.capacity_bytes,
        },
        "backend_note": backend_note,
        "cache_rules": rules,
        "prewarm_urls": ir.prewarm_set,
        "worker_script": worker_script,
    });

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(output, &json)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Compiled PolicyIR → Cloudflare deployment scaffold")?;
    writeln!(out, "  Cache rules: {}", rules.len())?;
    writeln!(out, "  Prewarm URLs: {}", ir.prewarm_set.len())?;
    writeln!(
        out,
        "  Worker script: {}",
        if worker_script.is_some() {
            "yes (admission gate)"
        } else {
            "no"
        }
    )?;
    if score_map.is_some() {
        writeln!(out, "  Scores: populated from optimize output")?;
    }
    writeln!(out, "  Output → {}", output.display())?;

    Ok(())
}

fn compile_bypass_rules(rule: &BypassRule, rules: &mut Vec<serde_json::Value>) {
    match rule {
        BypassRule::None => {}
        BypassRule::SizeLimit { max_bytes } => {
            rules.push(serde_json::json!({
                "description": format!("Bypass cache for objects > {} bytes", max_bytes),
                "expression": format!("http.response.headers[\"content-length\"][0] gt \"{}\"", max_bytes),
                "action": "bypass_cache"
            }));
        }
        BypassRule::FreshnessRisk { threshold } => {
            // Map freshness risk to content-type bypass for high-churn types
            let desc = if *threshold <= 0.3 {
                "Bypass high-churn content (API responses)"
            } else {
                "Bypass moderate-churn content"
            };
            rules.push(serde_json::json!({
                "description": desc,
                "expression": "http.response.headers[\"content-type\"][0] eq \"application/json\"",
                "action": "bypass_cache",
                "_freshness_threshold": threshold
            }));
        }
        BypassRule::Any { rules: sub_rules } => {
            for sub in sub_rules {
                compile_bypass_rules(sub, rules);
            }
        }
    }
}

fn generate_admission_worker(
    threshold: f64,
    mode: &str,
    score_map: Option<&HashMap<String, f64>>,
) -> String {
    let scores_js = if let Some(map) = score_map {
        let entries: Vec<String> = map
            .iter()
            .filter(|(_, &v)| v > threshold)
            .map(|(k, v)| format!("  \"{k}\": {v:.4}"))
            .collect();
        format!("{{\n{}\n}}", entries.join(",\n"))
    } else {
        "{\n  // Run `qc optimize` and pass --scores policy.json to populate\n}".to_string()
    };

    format!(
        r#"// quant-cache admission gate Worker
// Mode: {mode}, threshold: {threshold}

const ADMISSION_SCORES = {scores_js};

export default {{
  async fetch(request, env) {{
    const url = new URL(request.url);
    const key = url.pathname + url.search;

    const score = ADMISSION_SCORES[key];
    if (score === undefined || score <= {threshold}) {{
      return fetch(request, {{ cf: {{ cacheTtl: 0 }} }});
    }}

    return fetch(request);
  }}
}};
"#
    )
}
