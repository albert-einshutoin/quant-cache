use std::collections::HashMap;
use std::io::Write;

use qc_model::policy_ir::{AdmissionRule, BypassRule, PolicyIR};

pub(super) fn compile_cloudfront(
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

    // 3. Cache key normalization mapping
    let cache_key_config = compile_cache_key_config(&ir.cache_key_rules);

    // 4. CloudFront Function
    let function_code = match &ir.admission_rule {
        AdmissionRule::Always => None,
        AdmissionRule::ScoreThreshold { threshold }
        | AdmissionRule::ScoreDensityThreshold { threshold } => Some(gen_cf_function(
            *threshold,
            score_map,
            cache_key_config.as_ref(),
        )),
    };

    let config = serde_json::json!({
        "_generated_by": "quant-cache v0.3",
        "_target": "cloudfront",
        "_ir_summary": {
            "backend": format!("{:?}", ir.backend),
            "capacity_bytes": ir.capacity_bytes,
        },
        "cache_behaviors": cache_behaviors,
        "cache_key_config": cache_key_config,
        "prewarm_paths": ir.prewarm_set,
        "cloudfront_function": function_code,
        "_deploy_steps": [
            "1. Update distribution CacheBehaviors via AWS CLI or Console",
            "2. Apply cache_key_config to CloudFront Cache Policy / Function normalization logic",
            "3. If cloudfront_function is present, create CloudFront Function and associate",
            "4. Warm prewarm_paths via CloudFront invalidation or direct requests",
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

pub(super) fn content_type_to_cf_path(ct: &str) -> &str {
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

pub(super) fn compile_cache_key_config(
    cache_key_rules: &[qc_model::policy_ir::CacheKeyRule],
) -> Option<serde_json::Value> {
    if cache_key_rules.is_empty() {
        return None;
    }

    let query_params_to_strip: Vec<&str> = cache_key_rules
        .iter()
        .filter_map(|r| {
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
        "_note": "Map to CloudFront Cache Policy / Function query normalization",
        "_rules": cache_key_rules.iter().map(|r| {
            serde_json::json!({"pattern": &r.pattern, "replacement": &r.replacement})
        }).collect::<Vec<_>>()
    }))
}

pub(super) fn gen_cf_function(
    threshold: f64,
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
        "{ /* qc compile --scores policy.json */ }".to_string()
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
        r#"// quant-cache admission gate (CloudFront Function)
var SCORES = {scores_js};
var STRIP_PARAMS = [{strip_params}];

function shouldStripParam(name) {{
  for (var i = 0; i < STRIP_PARAMS.length; i++) {{
    var pattern = STRIP_PARAMS[i];
    if (pattern.endsWith('*')) {{
      if (name.startsWith(pattern.slice(0, -1))) return true;
    }} else if (name === pattern) {{
      return true;
    }}
  }}
  return false;
}}

function normalizedKey(request) {{
  var query = request.querystring || {{}};
  var parts = [];
  var names = Object.keys(query).sort();
  for (var i = 0; i < names.length; i++) {{
    var name = names[i];
    if (shouldStripParam(name)) continue;
    var entry = query[name];
    if (entry && typeof entry.value !== 'undefined') {{
      parts.push(name + '=' + entry.value);
    }}
  }}
  return parts.length ? request.uri + '?' + parts.join('&') : request.uri;
}}

function handler(event) {{
  var request = event.request;
  var key = normalizedKey(request);
  if (!SCORES[key] || SCORES[key] <= {threshold}) {{
    request.headers['x-qc-bypass'] = {{ value: 'true' }};
  }}
  return request;
}}
"#
    )
}

pub(super) fn validate_cloudfront(config: &serde_json::Value) -> Vec<String> {
    let mut issues = Vec::new();

    if config["_target"] != "cloudfront" {
        issues.push("_target should be 'cloudfront'".into());
    }

    // Validate cache behaviors
    if let Some(behaviors) = config["cache_behaviors"].as_array() {
        for (i, behavior) in behaviors.iter().enumerate() {
            let ctx = format!("cache_behaviors[{i}]");

            // DefaultTTL must be non-negative if present
            if let Some(ttl) = behavior["DefaultTTL"].as_i64() {
                if ttl < 0 {
                    issues.push(format!("{ctx}: DefaultTTL must be non-negative, got {ttl}"));
                }
            }

            // MaxTTL >= DefaultTTL
            if let (Some(default_ttl), Some(max_ttl)) =
                (behavior["DefaultTTL"].as_i64(), behavior["MaxTTL"].as_i64())
            {
                if max_ttl < default_ttl {
                    issues.push(format!(
                        "{ctx}: MaxTTL ({max_ttl}) < DefaultTTL ({default_ttl})"
                    ));
                }
            }

            // CachePolicyId format (UUID)
            if let Some(policy_id) = behavior["CachePolicyId"].as_str() {
                if policy_id.len() != 36 || policy_id.chars().filter(|&c| c == '-').count() != 4 {
                    issues.push(format!(
                        "{ctx}: CachePolicyId doesn't look like a UUID: '{policy_id}'"
                    ));
                }
            }
        }

        // CloudFront limit: max 25 cache behaviors per distribution
        if behaviors.len() > 25 {
            issues.push(format!(
                "too many cache behaviors: {} (CloudFront limit: 25)",
                behaviors.len()
            ));
        }
    }

    // Validate CloudFront Function if present
    if let Some(func) = config["cloudfront_function"].as_str() {
        // CloudFront Functions size limit: 10KB
        if func.len() > 10_240 {
            issues.push(format!(
                "CloudFront Function exceeds 10KB ({} bytes)",
                func.len()
            ));
        }
        if func.contains("/* qc compile --scores") || func.contains("/* populate") {
            issues.push("CloudFront Function contains unpopulated placeholder".into());
        }
    }

    // Validate prewarm paths
    if let Some(paths) = config["prewarm_paths"].as_array() {
        for (i, path) in paths.iter().enumerate() {
            if let Some(p) = path.as_str() {
                if !p.starts_with('/') {
                    issues.push(format!(
                        "prewarm_paths[{i}]: should start with '/', got '{p}'"
                    ));
                }
            }
        }
    }

    issues
}
