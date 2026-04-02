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
        "cloudfront" => compile_cloudfront(&ir, score_map.as_ref(), &args.output),
        other => anyhow::bail!("unsupported target: {other}. Supported: cloudflare, cloudfront"),
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

// ── CloudFront Compiler ─────────────────────────────────────────────

fn compile_cloudfront(
    ir: &PolicyIR,
    score_map: Option<&HashMap<String, f64>>,
    output: &std::path::Path,
) -> anyhow::Result<()> {
    let mut cache_behaviors = Vec::new();

    // 1. Bypass rules → CacheBehavior with CachePolicyId = CachingDisabled
    match &ir.bypass_rule {
        BypassRule::None => {}
        BypassRule::SizeLimit { max_bytes } => {
            cache_behaviors.push(serde_json::json!({
                "PathPattern": "# Objects > {} bytes — map to path patterns manually".replace("{}", &max_bytes.to_string()),
                "CachePolicyId": "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
                "_note": "CachingDisabled managed policy",
                "_bypass_reason": format!("size > {} bytes", max_bytes)
            }));
        }
        BypassRule::FreshnessRisk { threshold } => {
            cache_behaviors.push(serde_json::json!({
                "PathPattern": "/api/*",
                "CachePolicyId": "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
                "_note": "CachingDisabled for high-churn API content",
                "_freshness_threshold": threshold
            }));
        }
        BypassRule::Any { rules } => {
            for rule in rules {
                if let BypassRule::SizeLimit { max_bytes } = rule {
                    cache_behaviors.push(serde_json::json!({
                        "_bypass_reason": format!("size > {} bytes", max_bytes),
                        "CachePolicyId": "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"
                    }));
                }
                if let BypassRule::FreshnessRisk { .. } = rule {
                    cache_behaviors.push(serde_json::json!({
                        "PathPattern": "/api/*",
                        "CachePolicyId": "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
                        "_note": "CachingDisabled for high-churn content"
                    }));
                }
            }
        }
    }

    // 2. TTL class rules → CacheBehavior with custom TTL
    for rule in &ir.ttl_class_rules {
        let path_pattern = if rule.content_type_pattern.starts_with("image/") {
            "*.jpg;*.jpeg;*.png;*.gif;*.webp;*.avif;*.svg"
        } else if rule.content_type_pattern.starts_with("text/css")
            || rule
                .content_type_pattern
                .starts_with("application/javascript")
        {
            "*.css;*.js"
        } else if rule.content_type_pattern.starts_with("application/json") {
            "/api/*"
        } else {
            "*"
        };

        cache_behaviors.push(serde_json::json!({
            "PathPattern": path_pattern,
            "DefaultTTL": rule.ttl_seconds,
            "MaxTTL": rule.ttl_seconds * 2,
            "MinTTL": 0,
            "_content_type": rule.content_type_pattern
        }));
    }

    // 3. CloudFront Function for admission gate
    let function_code = match &ir.admission_rule {
        AdmissionRule::Always => None,
        AdmissionRule::ScoreThreshold { threshold }
        | AdmissionRule::ScoreDensityThreshold { threshold } => {
            let scores_js = if let Some(map) = score_map {
                let entries: Vec<String> = map
                    .iter()
                    .filter(|(_, &v)| v > *threshold)
                    .map(|(k, v)| format!("  '{k}': {v:.4}"))
                    .collect();
                format!("{{\n{}\n}}", entries.join(",\n"))
            } else {
                "{ /* populate from qc optimize output */ }".to_string()
            };

            Some(format!(
                r#"// quant-cache admission gate (CloudFront Function)
var SCORES = {scores_js};

function handler(event) {{
  var request = event.request;
  var key = request.uri;
  if (request.querystring) key += '?' + Object.entries(request.querystring).map(function(e) {{ return e[0] + '=' + (e[1].value || ''); }}).join('&');

  if (!SCORES[key] || SCORES[key] <= {threshold}) {{
    // Not admitted — add no-cache header
    request.headers['x-qc-bypass'] = {{ value: 'true' }};
  }}
  return request;
}}
"#
            ))
        }
    };

    let config = serde_json::json!({
        "_generated_by": "quant-cache",
        "_target": "cloudfront",
        "_policy_ir": {
            "backend": format!("{:?}", ir.backend),
            "capacity_bytes": ir.capacity_bytes,
        },
        "cache_behaviors": cache_behaviors,
        "prewarm_paths": ir.prewarm_set,
        "cloudfront_function": function_code,
        "_notes": [
            "CachePolicyId 4135ea2d... = AWS Managed CachingDisabled",
            "PathPattern values may need adjustment for your distribution",
            "Prewarm paths can be used with CloudFront invalidation/warm-up"
        ]
    });

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(output, &json)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Compiled PolicyIR → CloudFront deployment scaffold")?;
    writeln!(out, "  Cache behaviors: {}", cache_behaviors.len())?;
    writeln!(out, "  Prewarm paths: {}", ir.prewarm_set.len())?;
    writeln!(
        out,
        "  CloudFront Function: {}",
        if function_code.is_some() {
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
