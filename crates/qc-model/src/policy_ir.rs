use serde::{Deserialize, Serialize};

/// Cache eviction backend algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Sieve,
    S3Fifo,
}

/// Rule for admitting objects into cache on miss.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdmissionRule {
    /// Admit everything (pure eviction policy).
    #[default]
    Always,
    /// Admit if economic score > threshold.
    ScoreThreshold { threshold: f64 },
    /// Admit if score / size_bytes > threshold (density-based).
    ScoreDensityThreshold { threshold: f64 },
}

/// Rule for bypassing cache entirely (never attempt to cache).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BypassRule {
    /// Never bypass.
    #[default]
    None,
    /// Bypass objects larger than max_bytes.
    SizeLimit { max_bytes: u64 },
    /// Bypass objects whose freshness risk P(stale) exceeds threshold.
    FreshnessRisk { threshold: f64 },
    /// Combine multiple bypass conditions (any match = bypass).
    Any { rules: Vec<BypassRule> },
}

/// TTL class override rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtlClassRule {
    /// Content-type prefix pattern (e.g. "image/", "application/json").
    pub content_type_pattern: String,
    /// TTL override in seconds.
    pub ttl_seconds: u64,
}

/// Cache key transformation rule (evaluated in Phase C).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheKeyRule {
    /// Regex pattern to match against the original cache key.
    pub pattern: String,
    /// Replacement string.
    pub replacement: String,
}

/// The Policy Intermediate Representation.
///
/// A declarative, serializable description of a complete cache policy
/// that can be evaluated against a trace or compiled to vendor config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyIR {
    /// Eviction backend algorithm.
    pub backend: Backend,
    /// Cache capacity in bytes.
    pub capacity_bytes: u64,
    /// Admission rule (requires economic scores at evaluation time).
    #[serde(default)]
    pub admission_rule: AdmissionRule,
    /// Bypass rule (applied before admission check).
    #[serde(default)]
    pub bypass_rule: BypassRule,
    /// Objects to pre-warm (by cache_key). Inserted before trace replay.
    #[serde(default)]
    pub prewarm_set: Vec<String>,
    /// TTL class overrides by content type.
    #[serde(default)]
    pub ttl_class_rules: Vec<TtlClassRule>,
    /// Cache key transformation rules (Phase C — not evaluated in replay).
    #[serde(default)]
    pub cache_key_rules: Vec<CacheKeyRule>,
}
