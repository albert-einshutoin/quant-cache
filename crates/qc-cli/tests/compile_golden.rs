use std::process::Command;

fn qc() -> &'static str {
    env!("CARGO_BIN_EXE_qc")
}

fn write_ir(dir: &std::path::Path, name: &str, json: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, json).unwrap();
    path
}

#[test]
fn cloudflare_minimal_ir() {
    let dir = tempfile::tempdir().unwrap();
    let ir = write_ir(
        dir.path(),
        "ir.json",
        r#"{"backend":"sieve","capacity_bytes":100000}"#,
    );
    let out = dir.path().join("cf.json");

    let result = Command::new(qc())
        .args(["compile", "-p"])
        .arg(&ir)
        .args(["-t", "cloudflare", "-o"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();

    // Rulesets API structure
    assert_eq!(
        config["ruleset_payload"]["phase"],
        "http_request_cache_settings"
    );
    assert_eq!(config["ruleset_payload"]["kind"], "zone");
    assert!(config["ruleset_payload"]["rules"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(config["worker_script"].is_null());
    assert_eq!(config["_target"], "cloudflare");
}

#[test]
fn cloudflare_full_ir() {
    let dir = tempfile::tempdir().unwrap();
    let ir = write_ir(
        dir.path(),
        "ir.json",
        r#"{
        "backend": "sieve",
        "capacity_bytes": 50000000,
        "admission_rule": {"type": "score_threshold", "threshold": 1.0},
        "bypass_rule": {"type": "size_limit", "max_bytes": 10000000},
        "ttl_class_rules": [
            {"content_type_pattern": "image/", "ttl_seconds": 7200}
        ],
        "prewarm_set": ["/hero.jpg"]
    }"#,
    );
    let out = dir.path().join("cf.json");

    let result = Command::new(qc())
        .args(["compile", "-p"])
        .arg(&ir)
        .args(["-t", "cloudflare", "-o"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(result.status.success());

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let rules = config["ruleset_payload"]["rules"].as_array().unwrap();

    // Should have bypass rule + TTL rule = 2 rules
    assert_eq!(rules.len(), 2, "expected 2 rules, got {}", rules.len());

    // First rule: bypass
    assert_eq!(rules[0]["action"], "set_cache_settings");
    assert_eq!(rules[0]["action_parameters"]["cache"], false);

    // Second rule: TTL
    assert_eq!(rules[1]["action"], "set_cache_settings");
    assert_eq!(rules[1]["action_parameters"]["edge_ttl"]["default"], 7200);

    // Worker script present (admission gate)
    assert!(config["worker_script"].is_string());
    assert!(config["worker_script"]
        .as_str()
        .unwrap()
        .contains("threshold"));

    // Prewarm
    assert_eq!(config["prewarm_urls"][0], "/hero.jpg");

    // Deploy steps
    assert!(config["_deploy_steps"].as_array().unwrap().len() >= 2);
}

#[test]
fn cloudfront_full_ir() {
    let dir = tempfile::tempdir().unwrap();
    let ir = write_ir(
        dir.path(),
        "ir.json",
        r#"{
        "backend": "s3_fifo",
        "capacity_bytes": 50000000,
        "bypass_rule": {"type": "freshness_risk", "threshold": 0.3},
        "ttl_class_rules": [
            {"content_type_pattern": "application/json", "ttl_seconds": 300}
        ],
        "prewarm_set": ["/api/config"]
    }"#,
    );
    let out = dir.path().join("cfn.json");

    let result = Command::new(qc())
        .args(["compile", "-p"])
        .arg(&ir)
        .args(["-t", "cloudfront", "-o"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(result.status.success());

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();

    assert_eq!(config["_target"], "cloudfront");
    let behaviors = config["cache_behaviors"].as_array().unwrap();
    assert!(behaviors.len() >= 2, "bypass + TTL");

    // CachingDisabled policy ID for bypass
    assert!(behaviors
        .iter()
        .any(|b| b["CachePolicyId"] == "4135ea2d-6df8-44a3-9df3-4b5a84be39ad"));

    // TTL behavior
    assert!(behaviors.iter().any(|b| b["DefaultTTL"] == 300));

    assert_eq!(config["prewarm_paths"][0], "/api/config");
    assert!(config["_deploy_steps"].as_array().unwrap().len() >= 2);
}

#[test]
fn compile_with_scores() {
    let dir = tempfile::tempdir().unwrap();

    // Generate trace and optimize to get scores
    let trace = dir.path().join("trace.csv");
    let policy = dir.path().join("policy.json");

    let r = Command::new(qc())
        .args([
            "generate",
            "--num-objects",
            "20",
            "--num-requests",
            "500",
            "-o",
        ])
        .arg(&trace)
        .output()
        .unwrap();
    assert!(r.status.success());

    let r = Command::new(qc())
        .args(["optimize", "-i"])
        .arg(&trace)
        .args(["-o"])
        .arg(&policy)
        .args(["--capacity", "50000", "--preset", "ecommerce"])
        .output()
        .unwrap();
    assert!(r.status.success());

    // Compile with scores
    let ir = write_ir(
        dir.path(),
        "ir.json",
        r#"{
        "backend": "sieve",
        "capacity_bytes": 50000,
        "admission_rule": {"type": "score_threshold", "threshold": 0.1}
    }"#,
    );
    let out = dir.path().join("cf.json");

    let r = Command::new(qc())
        .args(["compile", "-p"])
        .arg(&ir)
        .args(["--scores"])
        .arg(&policy)
        .args(["-t", "cloudflare", "-o"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(r.status.success());

    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let worker = config["worker_script"].as_str().unwrap();

    // Worker should contain actual score values (not placeholder)
    assert!(
        !worker.contains("populate"),
        "scores should be populated, not placeholder"
    );
    assert!(
        worker.contains("/content/"),
        "should contain object paths from optimize"
    );
}
