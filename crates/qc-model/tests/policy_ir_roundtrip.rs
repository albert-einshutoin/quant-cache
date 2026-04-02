use qc_model::policy_ir::*;

fn roundtrip_json<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) {
    let json = serde_json::to_string_pretty(value).expect("serialize");
    let _: T = serde_json::from_str(&json).expect("deserialize");
}

#[test]
fn minimal_policy_ir() {
    let ir: PolicyIR =
        serde_json::from_str(r#"{"backend": "sieve", "capacity_bytes": 100000}"#).unwrap();
    assert_eq!(ir.backend, Backend::Sieve);
    assert_eq!(ir.capacity_bytes, 100000);
    assert!(matches!(ir.admission_rule, AdmissionRule::Always));
    assert!(matches!(ir.bypass_rule, BypassRule::None));
    assert!(ir.prewarm_set.is_empty());
    assert!(ir.ttl_class_rules.is_empty());
}

#[test]
fn full_policy_ir_roundtrip() {
    let ir = PolicyIR {
        backend: Backend::S3Fifo,
        capacity_bytes: 50_000_000,
        admission_rule: AdmissionRule::ScoreDensityThreshold { threshold: 0.001 },
        bypass_rule: BypassRule::Any {
            rules: vec![
                BypassRule::SizeLimit {
                    max_bytes: 10_000_000,
                },
                BypassRule::FreshnessRisk { threshold: 0.5 },
            ],
        },
        prewarm_set: vec!["/hero.jpg".into(), "/main.css".into()],
        ttl_class_rules: vec![
            TtlClassRule {
                content_type_pattern: "image/".into(),
                ttl_seconds: 7200,
            },
            TtlClassRule {
                content_type_pattern: "application/json".into(),
                ttl_seconds: 300,
            },
        ],
        cache_key_rules: vec![CacheKeyRule {
            pattern: "utm_.*".into(),
            replacement: "".into(),
        }],
    };
    roundtrip_json(&ir);
}

#[test]
fn toml_roundtrip() {
    let ir = PolicyIR {
        backend: Backend::Sieve,
        capacity_bytes: 100_000,
        admission_rule: AdmissionRule::ScoreThreshold { threshold: 1.0 },
        bypass_rule: BypassRule::None,
        prewarm_set: vec![],
        ttl_class_rules: vec![],
        cache_key_rules: vec![],
    };
    let toml_str = toml::to_string_pretty(&ir).expect("toml serialize");
    let back: PolicyIR = toml::from_str(&toml_str).expect("toml deserialize");
    assert_eq!(back.backend, Backend::Sieve);
}

#[test]
fn all_backends() {
    for backend_str in &["sieve", "s3_fifo"] {
        let json = format!(
            r#"{{"backend": "{}", "capacity_bytes": 1000}}"#,
            backend_str
        );
        let ir: PolicyIR = serde_json::from_str(&json).unwrap();
        roundtrip_json(&ir);
    }
}

#[test]
fn all_admission_rules() {
    let rules = vec![
        r#"{"type": "always"}"#,
        r#"{"type": "score_threshold", "threshold": 0.5}"#,
        r#"{"type": "score_density_threshold", "threshold": 0.001}"#,
    ];
    for rule_json in rules {
        let _: AdmissionRule = serde_json::from_str(rule_json).unwrap();
    }
}

#[test]
fn all_bypass_rules() {
    let rules = vec![
        r#"{"type": "none"}"#,
        r#"{"type": "size_limit", "max_bytes": 5000000}"#,
        r#"{"type": "freshness_risk", "threshold": 0.3}"#,
        r#"{"type": "any", "rules": [{"type": "size_limit", "max_bytes": 1000}, {"type": "freshness_risk", "threshold": 0.5}]}"#,
    ];
    for rule_json in rules {
        let _: BypassRule = serde_json::from_str(rule_json).unwrap();
    }
}
