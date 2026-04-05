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
            k
        };

        let mut map: HashMap<String, f64> = HashMap::new();
        for d in &pf.decisions {
            let nk = normalize(&d.cache_key);
            let entry = map.entry(nk).or_insert(0.0);
            if d.score > *entry {
                *entry = d.score;
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
        let safe_map: HashMap<&str, f64> = map
            .iter()
            .filter(|(_, &v)| v > threshold)
            .map(|(k, &v)| (k.as_str(), (v * 10000.0).round() / 10000.0))
            .collect();
        serde_json::to_string_pretty(&safe_map).unwrap_or_else(|_| "{}".into())
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

fn compile_cache_key_config(
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

fn gen_cf_function(
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

// ── Validators ──────────────────────────────────────────────────────

fn validate_cloudflare(config: &serde_json::Value) -> Vec<String> {
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

fn validate_cloudfront(config: &serde_json::Value) -> Vec<String> {
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

// ── Fastly VCL Compiler ─────────────────────────────────────────────

fn compile_fastly(
    ir: &PolicyIR,
    score_map: Option<&HashMap<String, f64>>,
    output: &std::path::Path,
) -> anyhow::Result<()> {
    let mut vcl_snippets = Vec::new();

    // 1. Bypass rules → vcl_recv pass
    let bypass_vcl = compile_fastly_bypass(&ir.bypass_rule);
    if !bypass_vcl.is_empty() {
        vcl_snippets.push(serde_json::json!({
            "name": "qc-bypass",
            "type": "recv",
            "priority": 10,
            "content": bypass_vcl
        }));
    }

    // 2. TTL rules → vcl_fetch override
    let mut ttl_lines = Vec::new();
    for rule in &ir.ttl_class_rules {
        let condition = if rule.content_type_pattern.ends_with('/') {
            format!(
                "beresp.http.Content-Type ~ \"^{}\"",
                rule.content_type_pattern
            )
        } else {
            format!(
                "beresp.http.Content-Type == \"{}\"",
                rule.content_type_pattern
            )
        };
        ttl_lines.push(format!(
            "  if ({condition}) {{\n    set beresp.ttl = {}s;\n  }}",
            rule.ttl_seconds
        ));
    }
    if !ttl_lines.is_empty() {
        vcl_snippets.push(serde_json::json!({
            "name": "qc-ttl-override",
            "type": "fetch",
            "priority": 10,
            "content": ttl_lines.join("\n")
        }));
    }

    // 3. Admission gate → vcl_recv with lookup table
    let admission_vcl = match &ir.admission_rule {
        AdmissionRule::Always => None,
        AdmissionRule::ScoreThreshold { threshold }
        | AdmissionRule::ScoreDensityThreshold { threshold } => {
            let table_entries: Vec<String> = if let Some(map) = score_map {
                map.iter()
                    .filter(|(_, &v)| v > *threshold)
                    .map(|(k, _)| {
                        let escaped = k
                            .replace('\\', "\\\\")
                            .replace('"', "\\\"")
                            .replace('\n', "\\n")
                            .replace('\r', "\\r")
                            .replace('\t', "\\t");
                        format!("  \"{escaped}\": \"1\"")
                    })
                    .collect()
            } else {
                vec!["  # Run: qc compile --scores policy.json to populate".into()]
            };

            let table = format!(
                "table qc_admission_scores {{\n{}\n}}",
                table_entries.join(",\n")
            );
            let recv = r#"if (!table.lookup(qc_admission_scores, req.url)) {
    return(pass);
  }"#
            .to_string();

            Some((table, recv))
        }
    };

    if let Some((table, recv_snippet)) = &admission_vcl {
        vcl_snippets.push(serde_json::json!({
            "name": "qc-admission-table",
            "type": "none",
            "priority": 5,
            "content": table
        }));
        vcl_snippets.push(serde_json::json!({
            "name": "qc-admission-gate",
            "type": "recv",
            "priority": 20,
            "content": recv_snippet
        }));
    }

    // Cache key rules → vcl_hash
    let mut key_vcl_lines: Vec<String> = Vec::new();
    for rule in &ir.cache_key_rules {
        if rule.pattern.contains("utm_") {
            key_vcl_lines
                .push("  set req.url = regsuball(req.url, \"[?&]utm_[^&]*\", \"\");".into());
        }
        if rule.pattern.contains("fbclid") {
            key_vcl_lines
                .push("  set req.url = regsuball(req.url, \"[?&]fbclid=[^&]*\", \"\");".into());
        }
    }
    if !key_vcl_lines.is_empty() {
        vcl_snippets.push(serde_json::json!({
            "name": "qc-cache-key-normalize",
            "type": "recv",
            "priority": 5,
            "content": key_vcl_lines.join("\n")
        }));
    }

    let config = serde_json::json!({
        "_generated_by": "quant-cache v0.5",
        "_target": "fastly",
        "_ir_summary": {
            "backend": format!("{:?}", ir.backend),
            "capacity_bytes": ir.capacity_bytes,
        },
        "vcl_snippets": vcl_snippets,
        "prewarm_urls": ir.prewarm_set,
        "_deploy_steps": [
            "1. Add VCL snippets via Fastly API or CLI (fastly vcl snippet create)",
            "2. If admission table is present, create edge dictionary or VCL table",
            "3. Activate new service version",
            "4. Warm prewarm_urls via direct requests",
        ]
    });

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(output, &json)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Compiled PolicyIR → Fastly VCL deployment scaffold")?;
    writeln!(out, "  VCL snippets: {}", vcl_snippets.len())?;
    writeln!(out, "  Prewarm URLs: {}", ir.prewarm_set.len())?;
    if admission_vcl.is_some() {
        writeln!(out, "  Admission table: yes")?;
    }
    if score_map.is_some() {
        writeln!(out, "  Scores: populated from optimize output")?;
    }
    writeln!(out, "  Output → {}", output.display())?;
    Ok(())
}

fn compile_fastly_bypass(rule: &BypassRule) -> String {
    match rule {
        BypassRule::None => String::new(),
        BypassRule::SizeLimit { max_bytes } => {
            format!("  if (std.atoi(beresp.http.Content-Length) > {max_bytes}) {{\n    set beresp.ttl = 0s;\n    set beresp.uncacheable = true;\n  }}")
        }
        BypassRule::FreshnessRisk { .. } => {
            "  if (req.url ~ \"^/api/\") {\n    return(pass);\n  }".into()
        }
        BypassRule::Any { rules } => rules
            .iter()
            .map(compile_fastly_bypass)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

// ── Akamai Property Manager Compiler ───────────────────────────────

fn compile_akamai(
    ir: &PolicyIR,
    score_map: Option<&HashMap<String, f64>>,
    output: &std::path::Path,
) -> anyhow::Result<()> {
    let mut children = Vec::new();

    // 1. Cache key normalization → cacheKeyQueryParams behavior
    let strip_params = extract_strip_params(&ir.cache_key_rules);
    if !strip_params.is_empty() {
        children.push(serde_json::json!({
            "name": "qc: cache key normalization",
            "criteria": [],
            "behaviors": [{
                "name": "cacheKeyQueryParams",
                "options": {
                    "behavior": "IGNORE",
                    "parameters": strip_params,
                    "exactMatch": false
                }
            }],
            "children": [],
            "criteriaMustSatisfy": "all"
        }));
    }

    // 2. Bypass rules → caching NO_STORE child rules
    compile_akamai_bypass(&ir.bypass_rule, &mut children);

    // 3. TTL class rules → caching MAX_AGE child rules
    for rule in &ir.ttl_class_rules {
        let ct_values = if rule.content_type_pattern.ends_with('/') {
            vec![format!("{}*", rule.content_type_pattern)]
        } else {
            vec![rule.content_type_pattern.clone()]
        };

        children.push(serde_json::json!({
            "name": format!("qc: TTL {} → {}s", rule.content_type_pattern, rule.ttl_seconds),
            "criteria": [{
                "name": "contentType",
                "options": {
                    "matchOperator": "IS_ONE_OF",
                    "matchWildcard": true,
                    "matchCaseSensitive": false,
                    "values": ct_values
                }
            }],
            "behaviors": [{
                "name": "caching",
                "options": {
                    "behavior": "MAX_AGE",
                    "mustRevalidate": false,
                    "ttl": seconds_to_akamai_ttl(rule.ttl_seconds)
                }
            }],
            "children": [],
            "criteriaMustSatisfy": "all"
        }));
    }

    // 4. Admission gate → EdgeWorker behavior
    let edgeworker = match &ir.admission_rule {
        AdmissionRule::Always => None,
        AdmissionRule::ScoreThreshold { threshold } => {
            Some(gen_akamai_edgeworker(*threshold, "score", score_map))
        }
        AdmissionRule::ScoreDensityThreshold { threshold } => {
            Some(gen_akamai_edgeworker(*threshold, "density", score_map))
        }
    };

    if edgeworker.is_some() {
        children.push(serde_json::json!({
            "name": "qc: admission gate",
            "criteria": [{
                "name": "requestType",
                "options": {
                    "matchOperator": "IS",
                    "value": "CLIENT_REQ"
                }
            }],
            "behaviors": [{
                "name": "edgeWorker",
                "options": {
                    "enabled": true,
                    "edgeWorkerId": "{{QC_EDGEWORKER_ID}}"
                }
            }],
            "children": [],
            "criteriaMustSatisfy": "all"
        }));
    }

    // Assemble PAPI rule tree
    let rule_tree = serde_json::json!({
        "rules": {
            "name": "default",
            "comments": "Generated by quant-cache",
            "criteria": [],
            "behaviors": [
                {
                    "name": "caching",
                    "options": {
                        "behavior": "MAX_AGE",
                        "mustRevalidate": false,
                        "ttl": "1d"
                    }
                },
                {
                    "name": "cacheKeyQueryParams",
                    "options": {
                        "behavior": "INCLUDE_ALL_ALPHABETIZE_ORDER"
                    }
                }
            ],
            "children": children,
            "criteriaMustSatisfy": "all",
            "options": { "is_secure": true }
        }
    });

    let config = serde_json::json!({
        "_generated_by": "quant-cache v0.5",
        "_target": "akamai",
        "_ir_summary": {
            "backend": format!("{:?}", ir.backend),
            "capacity_bytes": ir.capacity_bytes,
        },
        "rule_tree": rule_tree,
        "edgeworker_bundle": edgeworker,
        "prewarm_urls": ir.prewarm_set,
        "_deploy_steps": [
            "1. Create new property version via PAPI: POST /papi/v1/properties/{propertyId}/versions",
            "2. PUT rule tree to /papi/v1/properties/{propertyId}/versions/{version}/rules",
            "3. If edgeworker_bundle is present: create EW via EdgeWorkers API, replace {{QC_EDGEWORKER_ID}}",
            "4. Activate version: POST /papi/v1/activations (requires EdgeGrid auth)",
            "5. Warm prewarm_urls via Akamai Fast Purge API or direct requests",
        ]
    });

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(output, &json)?;

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "Compiled PolicyIR → Akamai Property Manager rule tree")?;
    writeln!(out, "  Child rules: {}", children.len())?;
    writeln!(out, "  Prewarm URLs: {}", ir.prewarm_set.len())?;
    writeln!(
        out,
        "  EdgeWorker: {}",
        if edgeworker.is_some() {
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

fn compile_akamai_bypass(rule: &BypassRule, children: &mut Vec<serde_json::Value>) {
    match rule {
        BypassRule::None => {}
        BypassRule::SizeLimit { max_bytes } => {
            // Akamai PAPI has no native response-size criterion.
            // Emit as advisory-only (not an active rule) to avoid disabling
            // cache for all traffic. Size-based bypass requires an EdgeWorker
            // that checks Content-Length and sets a bypass header.
            children.push(serde_json::json!({
                "name": format!("qc: bypass objects > {} bytes (ADVISORY)", max_bytes),
                "criteria": [{
                    "name": "requestHeader",
                    "options": {
                        "headerName": "X-QC-Size-Bypass",
                        "matchOperator": "IS_ONE_OF",
                        "values": ["true"],
                        "matchWildcardName": false,
                        "matchCaseSensitiveValue": false
                    }
                }],
                "behaviors": [
                    {
                        "name": "caching",
                        "options": { "behavior": "NO_STORE" }
                    }
                ],
                "children": [],
                "criteriaMustSatisfy": "all",
                "_note": format!(
                    "Akamai PAPI has no native content-length criterion. \
                     Deploy an EdgeWorker that sets X-QC-Size-Bypass: true \
                     for responses > {} bytes, then this rule activates.",
                    max_bytes
                )
            }));
        }
        BypassRule::FreshnessRisk { .. } => {
            children.push(serde_json::json!({
                "name": "qc: bypass high-churn API content",
                "criteria": [{
                    "name": "contentType",
                    "options": {
                        "matchOperator": "IS_ONE_OF",
                        "matchWildcard": false,
                        "matchCaseSensitive": false,
                        "values": ["application/json"]
                    }
                }],
                "behaviors": [
                    { "name": "caching", "options": { "behavior": "NO_STORE" } },
                    { "name": "downstreamCache", "options": { "behavior": "BUST" } }
                ],
                "children": [],
                "criteriaMustSatisfy": "all"
            }));
        }
        BypassRule::Any { rules } => {
            for r in rules {
                compile_akamai_bypass(r, children);
            }
        }
    }
}

fn seconds_to_akamai_ttl(s: u64) -> String {
    if s >= 86400 && s % 86400 == 0 {
        format!("{}d", s / 86400)
    } else if s >= 3600 && s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s >= 60 && s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{}s", s)
    }
}

fn gen_akamai_edgeworker(
    threshold: f64,
    mode: &str,
    score_map: Option<&HashMap<String, f64>>,
) -> String {
    let scores_js = if let Some(map) = score_map {
        // Use JSON serialization to safely escape keys (prevents JS injection)
        let safe_map: HashMap<&str, f64> = map
            .iter()
            .filter(|(_, &v)| v > threshold)
            .map(|(k, &v)| (k.as_str(), (v * 10000.0).round() / 10000.0))
            .collect();
        serde_json::to_string_pretty(&safe_map).unwrap_or_else(|_| "{}".into())
    } else {
        "{ /* Run: qc compile --scores policy.json to populate */ }".to_string()
    };

    format!(
        r#"// quant-cache admission gate EdgeWorker
// Mode: {mode}, threshold: {threshold}

const SCORES = {scores_js};

export function onClientRequest(request) {{
  const key = request.path + (request.query ? '?' + request.query : '');
  const score = SCORES[key];
  if (score === undefined || score <= {threshold}) {{
    // Sub-threshold: bypass cache
    request.setHeader('X-QC-Bypass', 'true');
    request.route({{ origin: 'default', cacheKey: null }});
  }}
}}
"#
    )
}

fn extract_strip_params(cache_key_rules: &[qc_model::policy_ir::CacheKeyRule]) -> Vec<&str> {
    let mut params = Vec::new();
    for r in cache_key_rules {
        if r.pattern.contains("utm_") {
            params.extend([
                "utm_source",
                "utm_medium",
                "utm_campaign",
                "utm_content",
                "utm_term",
            ]);
        }
        if r.pattern.contains("fbclid") {
            params.push("fbclid");
        }
    }
    params.sort();
    params.dedup();
    params
}

fn validate_fastly(config: &serde_json::Value) -> Vec<String> {
    let mut issues = Vec::new();

    if config["_target"] != "fastly" {
        issues.push("_target should be 'fastly'".into());
    }

    if let Some(snippets) = config["vcl_snippets"].as_array() {
        for (i, snippet) in snippets.iter().enumerate() {
            let ctx = format!("vcl_snippets[{i}]");

            if snippet["name"].as_str().unwrap_or("").is_empty() {
                issues.push(format!("{ctx}: missing 'name'"));
            }

            let stype = snippet["type"].as_str().unwrap_or("");
            let valid_types = [
                "init", "recv", "hash", "hit", "miss", "pass", "fetch", "deliver", "log", "error",
            ];
            if !valid_types.contains(&stype) {
                issues.push(format!("{ctx}: invalid VCL snippet type '{stype}'"));
            }

            if snippet["content"].as_str().unwrap_or("").is_empty() {
                issues.push(format!("{ctx}: empty content"));
            }

            // Fastly VCL snippet size limit: 64KB
            if let Some(content) = snippet["content"].as_str() {
                if content.len() > 65_536 {
                    issues.push(format!(
                        "{ctx}: content exceeds 64KB ({} bytes)",
                        content.len()
                    ));
                }
                if content.contains("# Run: qc compile --scores") {
                    issues.push(format!("{ctx}: contains unpopulated placeholder"));
                }
            }
        }

        // Fastly limit: max 100 snippets per service
        if snippets.len() > 100 {
            issues.push(format!(
                "too many VCL snippets: {} (Fastly limit: 100)",
                snippets.len()
            ));
        }
    }

    if let Some(urls) = config["prewarm_urls"].as_array() {
        for (i, url) in urls.iter().enumerate() {
            if let Some(u) = url.as_str() {
                if !u.starts_with('/') {
                    issues.push(format!("prewarm_urls[{i}]: should start with '/'"));
                }
            }
        }
    }

    issues
}

fn validate_akamai(config: &serde_json::Value) -> Vec<String> {
    let mut issues = Vec::new();

    if config["_target"] != "akamai" {
        issues.push("_target should be 'akamai'".into());
    }

    let rule_tree = &config["rule_tree"];
    if rule_tree.is_null() {
        issues.push("missing 'rule_tree' in output".into());
        return issues;
    }

    let root = &rule_tree["rules"];
    if root.is_null() {
        issues.push("missing 'rule_tree.rules' root".into());
        return issues;
    }

    if root["name"] != "default" {
        issues.push(format!(
            "root rule name should be 'default', got {:?}",
            root["name"]
        ));
    }

    if let Some(children) = root["children"].as_array() {
        for (i, child) in children.iter().enumerate() {
            let ctx = format!("children[{i}]");

            if child["name"].as_str().unwrap_or("").is_empty() {
                issues.push(format!("{ctx}: missing 'name'"));
            }

            if let Some(behaviors) = child["behaviors"].as_array() {
                for (j, behavior) in behaviors.iter().enumerate() {
                    if behavior["name"].as_str().unwrap_or("").is_empty() {
                        issues.push(format!("{ctx}.behaviors[{j}]: missing behavior 'name'"));
                    }
                }
                // Check for unreplaced EdgeWorker placeholder
                for b in behaviors {
                    if b["name"] == "edgeWorker" {
                        if let Some(ew_id) = b["options"]["edgeWorkerId"].as_str() {
                            if ew_id.contains("{{") {
                                issues.push(format!(
                                    "{ctx}: edgeWorkerId contains unreplaced placeholder '{ew_id}'"
                                ));
                            }
                        }
                    }
                }
            } else {
                issues.push(format!("{ctx}: missing 'behaviors' array"));
            }
        }

        if children.len() > 100 {
            issues.push(format!(
                "too many child rules: {} (recommended max: 100)",
                children.len()
            ));
        }
    }

    // Validate EdgeWorker bundle
    if let Some(bundle) = config["edgeworker_bundle"].as_str() {
        if bundle.contains("/* Run: qc compile --scores") || bundle.contains("/* populate") {
            issues.push("EdgeWorker bundle contains unpopulated placeholder scores".into());
        }
        if bundle.len() > 1_000_000 {
            issues.push(format!(
                "EdgeWorker bundle exceeds 1MB ({} bytes)",
                bundle.len()
            ));
        }
    }

    if let Some(urls) = config["prewarm_urls"].as_array() {
        for (i, url) in urls.iter().enumerate() {
            if let Some(u) = url.as_str() {
                if !u.starts_with('/') {
                    issues.push(format!("prewarm_urls[{i}]: should start with '/'"));
                }
            }
        }
    }

    issues
}
