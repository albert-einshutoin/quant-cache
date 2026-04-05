use serde::{Deserialize, Serialize};

/// Scoring version selection.
/// Serde aliases allow both short ("v1"/"v2") and full names in config files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScoringVersion {
    /// V1: frequency-based demand estimation (request_count).
    #[default]
    #[serde(rename = "v1", alias = "v1_frequency")]
    V1Frequency,
    /// V2: reuse-distance-aware hit probability estimation.
    #[serde(rename = "v2", alias = "v2_reuse_distance")]
    V2ReuseDistance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioConfig {
    pub capacity_bytes: u64,
    pub time_window_seconds: u64,
    /// Latency economic value ($/ms).
    pub latency_value_per_ms: f64,
    pub freshness_model: FreshnessModel,
    /// Scoring model version. Defaults to V1 (frequency-based).
    #[serde(default)]
    pub scoring_version: ScoringVersion,
}

/// Freshness model selection.
/// TTL-Only and InvalidationOnUpdate are mutually exclusive
/// to prevent double-counting of stale penalty and invalidation cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FreshnessModel {
    /// No invalidation. Stale penalty only.
    TtlOnly { stale_penalty: StalePenaltyConfig },
    /// Invalidation on every update. Stale ≈ 0.
    InvalidationOnUpdate { invalidation_cost: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StalePenaltyConfig {
    pub default_class: StalePenaltyClass,
    /// Optional per-class cost overrides ($/event).
    /// If not set, built-in defaults are used.
    #[serde(default)]
    pub cost_overrides: StaleCostOverrides,
}

/// Custom $/event values per stale penalty class.
/// All fields default to None (use built-in defaults).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StaleCostOverrides {
    pub none: Option<f64>,
    pub low: Option<f64>,
    pub medium: Option<f64>,
    pub high: Option<f64>,
    pub very_high: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StalePenaltyClass {
    None,
    Low,
    Medium,
    High,
    VeryHigh,
}

impl StalePenaltyClass {
    /// Map class to $/event value using built-in defaults.
    ///
    /// Default values (V1):
    /// - None: $0.00 — cacheable without risk (images, videos)
    /// - Low: $0.001 — minor freshness impact (CSS, JS)
    /// - Medium: $0.01 — moderate freshness impact (product metadata)
    /// - High: $0.10 — significant freshness impact (prices, inventory)
    /// - VeryHigh: $1.00 — critical (auth tokens, financial data)
    pub fn to_cost(self) -> f64 {
        match self {
            Self::None => 0.0,
            Self::Low => 0.001,
            Self::Medium => 0.01,
            Self::High => 0.1,
            Self::VeryHigh => 1.0,
        }
    }

    /// Map class to $/event using overrides if available, otherwise built-in defaults.
    pub fn to_cost_with_overrides(self, overrides: &StaleCostOverrides) -> f64 {
        match self {
            Self::None => overrides.none.unwrap_or(0.0),
            Self::Low => overrides.low.unwrap_or(0.001),
            Self::Medium => overrides.medium.unwrap_or(0.01),
            Self::High => overrides.high.unwrap_or(0.1),
            Self::VeryHigh => overrides.very_high.unwrap_or(1.0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityConstraint {
    pub capacity_bytes: u64,
}
