use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::policy_ir::{AdmissionRule, Backend, BypassRule, PolicyIR};

#[derive(Args)]
pub struct CompileArgs {
    /// PolicyIR JSON file
    #[arg(short, long)]
    pub policy: PathBuf,

    /// Target platform: cloudflare
    #[arg(short, long, default_value = "cloudflare")]
    pub target: String,

    /// Output file
    #[arg(short, long, default_value = "cache-config.json")]
    pub output: PathBuf,
}

pub fn run(args: &CompileArgs) -> anyhow::Result<()> {
    let ir_str = std::fs::read_to_string(&args.policy)?;
    let ir: PolicyIR = serde_json::from_str(&ir_str)?;

    match args.target.as_str() {
        "cloudflare" => compile_cloudflare(&ir, &args.output),
        other => anyhow::bail!("unsupported target: {other}. Supported: cloudflare"),
    }
}

fn compile_cloudflare(ir: &PolicyIR, output: &std::path::Path) -> anyhow::Result<()> {
    let mut rules = Vec::new();

    // 1. Bypass rules → Cloudflare Cache Rules with "bypass cache"
    match &ir.bypass_rule {
        BypassRule::None => {}
        BypassRule::SizeLimit { max_bytes } => {
            rules.push(serde_json::json!({
                "description": format!("Bypass cache for objects > {} bytes", max_bytes),
                "expression": format!("http.response.headers[\"content-length\"][0] gt \"{}\"", max_bytes),
                "action": "bypass_cache"
            }));
        }
        BypassRule::FreshnessRisk { threshold } => {
            rules.push(serde_json::json!({
                "description": format!("Bypass high-churn content (freshness_risk > {:.2})", threshold),
                "expression": "# Requires custom header or Worker to evaluate freshness risk",
                "action": "bypass_cache",
                "_note": format!("Map freshness_risk > {} to content-type or path rules", threshold)
            }));
        }
        BypassRule::Any { rules: sub_rules } => {
            for (i, rule) in sub_rules.iter().enumerate() {
                if let BypassRule::SizeLimit { max_bytes } = rule {
                    rules.push(serde_json::json!({
                        "description": format!("Bypass rule {} - size > {}", i + 1, max_bytes),
                        "expression": format!("http.response.headers[\"content-length\"][0] gt \"{}\"", max_bytes),
                        "action": "bypass_cache"
                    }));
                }
            }
        }
    }

    // 2. TTL class rules → Cloudflare Cache Rules with "set cache TTL"
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
            "action_parameters": {
                "cache_ttl": rule.ttl_seconds
            }
        }));
    }

    // 3. Admission rule → Cloudflare Worker script (if not Always)
    let worker_script = match &ir.admission_rule {
        AdmissionRule::Always => None,
        AdmissionRule::ScoreThreshold { threshold } => {
            Some(generate_admission_worker(*threshold, "score"))
        }
        AdmissionRule::ScoreDensityThreshold { threshold } => {
            Some(generate_admission_worker(*threshold, "density"))
        }
    };

    // 4. Backend recommendation
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
    writeln!(out, "Compiled PolicyIR → Cloudflare config")?;
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
    writeln!(out, "  Output → {}", output.display())?;

    Ok(())
}

fn generate_admission_worker(threshold: f64, mode: &str) -> String {
    format!(
        r#"// quant-cache admission gate Worker
// Mode: {mode}, threshold: {threshold}

const ADMISSION_SCORES = {{
  // Populated from qc optimize output
  // "/path/to/object": score_value,
}};

export default {{
  async fetch(request, env) {{
    const url = new URL(request.url);
    const key = url.pathname + url.search;

    const score = ADMISSION_SCORES[key];
    if (score === undefined || score <= {threshold}) {{
      // Not in admission set or below threshold — bypass cache
      return fetch(request, {{ cf: {{ cacheTtl: 0 }} }});
    }}

    // Admitted — let Cloudflare cache normally
    return fetch(request);
  }}
}};
"#
    )
}
