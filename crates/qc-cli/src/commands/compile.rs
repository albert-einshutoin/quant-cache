use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use clap::Args;

use qc_model::policy::PolicyFile;
use qc_model::policy_ir::{AdmissionRule, BypassRule, PolicyIR};

#[derive(Args)]
pub struct CompileArgs {
    /// PolicyIR JSON file
    #[arg(short, long)]
    pub policy: PathBuf,

    /// Target platform: cloudflare, cloudfront
    #[arg(short, long, default_value = "cloudflare")]
    pub target: String,

    /// Scores file (PolicyFile JSON from `qc optimize`) for admission gate
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

// ── Cloudflare Compiler ─────────────────────────────────────────────

/// Generate Cloudflare Rulesets API-compatible cache rules + Workers script.
fn compile_cloudflare(
    ir: &PolicyIR,
    score_map: Option<&HashMap<String, f64>>,
    output: &std::path::Path,
) -> anyhow::Result<()> {
    let mut rules = Vec::new();

    // 1. Bypass rules → Cloudflare Cache Rules
    compile_cf_bypass(&ir.bypass_rule, &mut rules);

    // 2. TTL class rules → Cloudflare Cache Rules (set_cache_settings)
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
            "expression": expression,
            "description": format!("qc: TTL {} → {}s", rule.content_type_pattern, rule.ttl_seconds),
            "action": "set_cache_settings",
            "action_parameters": {
                "edge_ttl": { "mode": "override_origin", "default": rule.ttl_seconds },
                "browser_ttl": { "mode": "override_origin", "default": rule.ttl_seconds / 2 }
            },
            "enabled": true
        }));
    }

    // 3. Cache key normalization rules
    let cache_key_config = if ir.cache_key_rules.is_empty() {
        None
    } else {
        let query_params_to_strip: Vec<&str> = ir
            .cache_key_rules
            .iter()
            .filter_map(|r| {
                // Extract param name from patterns like [?&]utm_[^&]* or [?&]fbclid=[^&]*
                if r.pattern.contains("utm_") {
                    Some("utm_*")
                } else if r.pattern.contains("fbclid") {
                    Some("fbclid")
                } else {
                    None
                }
            })
            .collect();

        Some(serde_json::json!({
            "query_string_strip": query_params_to_strip,
            "_note": "Map to Cloudflare Cache Key → Query String settings",
            "_rules": ir.cache_key_rules.iter().map(|r| {
                serde_json::json!({"pattern": &r.pattern, "replacement": &r.replacement})
            }).collect::<Vec<_>>()
        }))
    };

    // 4. Worker script for admission gate
    let worker = match &ir.admission_rule {
        AdmissionRule::Always => None,
        AdmissionRule::ScoreThreshold { threshold } => {
            Some(gen_cf_worker(*threshold, "score", score_map))
        }
        AdmissionRule::ScoreDensityThreshold { threshold } => {
            Some(gen_cf_worker(*threshold, "density", score_map))
        }
    };

    // Assemble Cloudflare Rulesets API payload
    let ruleset = serde_json::json!({
        "name": "quant-cache generated rules",
        "kind": "zone",
        "phase": "http_request_cache_settings",
        "rules": rules
    });

    let config = serde_json::json!({
        "_generated_by": "quant-cache v0.3",
        "_target": "cloudflare",
        "_ir_summary": {
            "backend": format!("{:?}", ir.backend),
            "capacity_bytes": ir.capacity_bytes,
            "admission": format!("{:?}", ir.admission_rule),
        },
        "ruleset_payload": ruleset,
        "cache_key_config": cache_key_config,
        "worker_script": worker,
        "prewarm_urls": ir.prewarm_set,
        "_deploy_steps": [
            "1. Create ruleset via PUT /zones/{zone_id}/rulesets/phases/http_request_cache_settings/entrypoint",
            "2. If worker_script is present, deploy via wrangler deploy",
            "3. Warm prewarm_urls via curl or Cloudflare API",
        ]
    });

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(output, &json)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Compiled PolicyIR → Cloudflare Rulesets API payload")?;
    writeln!(out, "  Cache rules: {}", rules.len())?;
    writeln!(out, "  Prewarm URLs: {}", ir.prewarm_set.len())?;
    writeln!(
        out,
        "  Worker: {}",
        if worker.is_some() {
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

fn compile_cf_bypass(rule: &BypassRule, rules: &mut Vec<serde_json::Value>) {
    match rule {
        BypassRule::None => {}
        BypassRule::SizeLimit { max_bytes } => {
            rules.push(serde_json::json!({
                "expression": format!(
                    "http.response.headers[\"content-length\"][0] gt \"{}\"",
                    max_bytes
                ),
                "description": format!("qc: bypass objects > {} bytes", max_bytes),
                "action": "set_cache_settings",
                "action_parameters": { "cache": false },
                "enabled": true
            }));
        }
        BypassRule::FreshnessRisk { .. } => {
            rules.push(serde_json::json!({
                "expression": "http.response.headers[\"content-type\"][0] eq \"application/json\"",
                "description": "qc: bypass high-churn API content",
                "action": "set_cache_settings",
                "action_parameters": { "cache": false },
                "enabled": true
            }));
        }
        BypassRule::Any { rules: sub } => {
            for r in sub {
                compile_cf_bypass(r, rules);
            }
        }
    }
}

fn gen_cf_worker(threshold: f64, mode: &str, score_map: Option<&HashMap<String, f64>>) -> String {
    let scores_js = if let Some(map) = score_map {
        let entries: Vec<String> = map
            .iter()
            .filter(|(_, &v)| v > threshold)
            .map(|(k, v)| format!("  \"{k}\": {v:.4}"))
            .collect();
        format!("{{\n{}\n}}", entries.join(",\n"))
    } else {
        "{\n  // Run: qc compile --scores <policy.json> to populate\n}".to_string()
    };

    format!(
        r#"// quant-cache admission gate Worker
// Mode: {mode}, threshold: {threshold}

const SCORES = {scores_js};

export default {{
  async fetch(request, env) {{
    const url = new URL(request.url);
    const key = url.pathname + url.search;
    const score = SCORES[key];
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

    // 1. Bypass rules
    match &ir.bypass_rule {
        BypassRule::None => {}
        BypassRule::SizeLimit { max_bytes } => {
            cache_behaviors.push(serde_json::json!({
                "_bypass_reason": format!("size > {} bytes — map to path patterns", max_bytes),
                "CachePolicyId": "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"
            }));
        }
        BypassRule::FreshnessRisk { .. } => {
            cache_behaviors.push(serde_json::json!({
                "PathPattern": "/api/*",
                "CachePolicyId": "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
                "_note": "CachingDisabled for high-churn content"
            }));
        }
        BypassRule::Any { rules } => {
            for r in rules {
                match r {
                    BypassRule::SizeLimit { max_bytes } => {
                        cache_behaviors.push(serde_json::json!({
                            "_bypass_reason": format!("size > {} bytes", max_bytes),
                            "CachePolicyId": "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"
                        }));
                    }
                    BypassRule::FreshnessRisk { .. } => {
                        cache_behaviors.push(serde_json::json!({
                            "PathPattern": "/api/*",
                            "CachePolicyId": "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"
                        }));
                    }
                    _ => {}
                }
            }
        }
    }

    // 2. TTL class rules
    for rule in &ir.ttl_class_rules {
        let path_pattern = content_type_to_cf_path(&rule.content_type_pattern);
        cache_behaviors.push(serde_json::json!({
            "PathPattern": path_pattern,
            "DefaultTTL": rule.ttl_seconds,
            "MaxTTL": rule.ttl_seconds * 2,
            "MinTTL": 0,
            "_content_type": rule.content_type_pattern
        }));
    }

    // 3. CloudFront Function
    let function_code = match &ir.admission_rule {
        AdmissionRule::Always => None,
        AdmissionRule::ScoreThreshold { threshold }
        | AdmissionRule::ScoreDensityThreshold { threshold } => {
            Some(gen_cf_function(*threshold, score_map))
        }
    };

    let config = serde_json::json!({
        "_generated_by": "quant-cache v0.3",
        "_target": "cloudfront",
        "_ir_summary": {
            "backend": format!("{:?}", ir.backend),
            "capacity_bytes": ir.capacity_bytes,
        },
        "cache_behaviors": cache_behaviors,
        "prewarm_paths": ir.prewarm_set,
        "cloudfront_function": function_code,
        "_deploy_steps": [
            "1. Update distribution CacheBehaviors via AWS CLI or Console",
            "2. If cloudfront_function is present, create CloudFront Function and associate",
            "3. Warm prewarm_paths via CloudFront invalidation or direct requests",
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
        "  Function: {}",
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

fn content_type_to_cf_path(ct: &str) -> &str {
    if ct.starts_with("image/") {
        "*.jpg;*.jpeg;*.png;*.gif;*.webp;*.avif;*.svg"
    } else if ct.starts_with("text/css") || ct.starts_with("application/javascript") {
        "*.css;*.js"
    } else if ct.starts_with("application/json") {
        "/api/*"
    } else {
        "*"
    }
}

fn gen_cf_function(threshold: f64, score_map: Option<&HashMap<String, f64>>) -> String {
    let scores_js = if let Some(map) = score_map {
        let entries: Vec<String> = map
            .iter()
            .filter(|(_, &v)| v > threshold)
            .map(|(k, v)| format!("  '{k}': {v:.4}"))
            .collect();
        format!("{{\n{}\n}}", entries.join(",\n"))
    } else {
        "{ /* qc compile --scores policy.json */ }".to_string()
    };

    format!(
        r#"// quant-cache admission gate (CloudFront Function)
var SCORES = {scores_js};
function handler(event) {{
  var request = event.request;
  var key = request.uri;
  if (!SCORES[key] || SCORES[key] <= {threshold}) {{
    request.headers['x-qc-bypass'] = {{ value: 'true' }};
  }}
  return request;
}}
"#
    )
}
