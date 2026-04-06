use std::collections::HashMap;
use std::io::Write;

use qc_model::policy_ir::{AdmissionRule, BypassRule, PolicyIR};

pub(super) fn compile_fastly(
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

pub(super) fn compile_fastly_bypass(rule: &BypassRule) -> String {
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

pub(super) fn validate_fastly(config: &serde_json::Value) -> Vec<String> {
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
