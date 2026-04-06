use std::collections::HashMap;
use std::io::Write;

use qc_model::policy_ir::{AdmissionRule, BypassRule, PolicyIR};

/// Generate Cloudflare Rulesets API-compatible cache rules + Workers script.
pub(super) fn compile_cloudflare(
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
        AdmissionRule::ScoreThreshold { threshold } => Some(gen_cf_worker(
            *threshold,
            "score",
            score_map,
            cache_key_config.as_ref(),
        )),
        AdmissionRule::ScoreDensityThreshold { threshold } => Some(gen_cf_worker(
            *threshold,
            "density",
            score_map,
            cache_key_config.as_ref(),
        )),
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

pub(super) fn compile_cf_bypass(rule: &BypassRule, rules: &mut Vec<serde_json::Value>) {
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

pub(super) fn gen_cf_worker(
    threshold: f64,
    mode: &str,
    score_map: Option<&HashMap<String, f64>>,
    cache_key_config: Option<&serde_json::Value>,
) -> String {
    let scores_js = if let Some(map) = score_map {
        let safe_map: HashMap<&str, f64> = map
            .iter()
            .filter(|(_, &v)| v > threshold)
            .map(|(k, &v)| (k.as_str(), (v * 10000.0).round() / 10000.0))
            .collect();
        serde_json::to_string_pretty(&safe_map).unwrap_or_else(|_| "{}".into())
    } else {
        "{\n  // Run: qc compile --scores <policy.json> to populate\n}".to_string()
    };

    let strip_params = cache_key_config
        .and_then(|cfg| cfg["query_string_strip"].as_array())
        .map(|params| {
            params
                .iter()
                .filter_map(|p| p.as_str())
                .map(|p| format!("'{p}'"))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    format!(
        r#"// quant-cache admission gate Worker
// Mode: {mode}, threshold: {threshold}

const SCORES = {scores_js};
const STRIP_PARAMS = [{strip_params}];

function shouldStripParam(name) {{
  for (let i = 0; i < STRIP_PARAMS.length; i++) {{
    const pattern = STRIP_PARAMS[i];
    if (pattern.endsWith('*')) {{
      if (name.startsWith(pattern.slice(0, -1))) return true;
    }} else if (name === pattern) {{
      return true;
    }}
  }}
  return false;
}}

function normalizedKey(url) {{
  const params = [];
  const sorted = Array.from(url.searchParams.keys()).sort();
  for (const name of sorted) {{
    if (shouldStripParam(name)) continue;
    params.push(name + '=' + url.searchParams.get(name));
  }}
  return params.length ? url.pathname + '?' + params.join('&') : url.pathname;
}}

export default {{
  async fetch(request, env) {{
    const url = new URL(request.url);
    const key = normalizedKey(url);
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

pub(super) fn validate_cloudflare(config: &serde_json::Value) -> Vec<String> {
    let mut issues = Vec::new();

    // Check ruleset_payload structure
    let ruleset = &config["ruleset_payload"];
    if ruleset.is_null() {
        issues.push("missing 'ruleset_payload' in output".into());
        return issues;
    }

    if ruleset["phase"] != "http_request_cache_settings" {
        issues.push(format!(
            "ruleset phase should be 'http_request_cache_settings', got {:?}",
            ruleset["phase"]
        ));
    }

    if ruleset["kind"] != "zone" {
        issues.push(format!(
            "ruleset kind should be 'zone', got {:?}",
            ruleset["kind"]
        ));
    }

    // Validate each rule
    if let Some(rules) = ruleset["rules"].as_array() {
        for (i, rule) in rules.iter().enumerate() {
            let ctx = format!("rule[{}]", i);

            // Must have expression
            if rule["expression"].as_str().unwrap_or("").is_empty() {
                issues.push(format!("{ctx}: missing or empty 'expression'"));
            }

            // Must have action
            let action = rule["action"].as_str().unwrap_or("");
            if action.is_empty() {
                issues.push(format!("{ctx}: missing 'action'"));
            } else if action != "set_cache_settings" {
                issues.push(format!(
                    "{ctx}: unexpected action '{action}' (expected 'set_cache_settings')"
                ));
            }

            // Must have action_parameters
            if rule["action_parameters"].is_null() {
                issues.push(format!("{ctx}: missing 'action_parameters'"));
            }

            // Must have enabled field
            if rule["enabled"].is_null() {
                issues.push(format!("{ctx}: missing 'enabled' field"));
            }

            // Validate description length (Cloudflare limit: 500 chars)
            if let Some(desc) = rule["description"].as_str() {
                if desc.len() > 500 {
                    issues.push(format!(
                        "{ctx}: description exceeds 500 chars ({})",
                        desc.len()
                    ));
                }
            }
        }

        // Cloudflare limit: max 25 rules per phase
        if rules.len() > 25 {
            issues.push(format!(
                "too many rules: {} (Cloudflare limit: 25 per phase)",
                rules.len()
            ));
        }
    }

    // Validate worker script if present
    if let Some(worker) = config["worker_script"].as_str() {
        if worker.contains("// Run: qc compile --scores") || worker.contains("/* populate") {
            issues.push("worker script contains unpopulated placeholder scores".into());
        }
        // Cloudflare Workers size limit: 1MB for bundled, 10MB for paid
        if worker.len() > 1_000_000 {
            issues.push(format!(
                "worker script exceeds 1MB ({} bytes)",
                worker.len()
            ));
        }
    }

    // Validate prewarm URLs
    if let Some(urls) = config["prewarm_urls"].as_array() {
        for (i, url) in urls.iter().enumerate() {
            if let Some(u) = url.as_str() {
                if !u.starts_with('/') {
                    issues.push(format!(
                        "prewarm_urls[{i}]: should start with '/', got '{u}'"
                    ));
                }
            }
        }
    }

    issues
}
