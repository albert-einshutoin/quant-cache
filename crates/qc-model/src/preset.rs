use crate::scenario::{
    FreshnessModel, ScenarioConfig, ScoringVersion, StaleCostOverrides, StalePenaltyClass,
    StalePenaltyConfig,
};

/// Preset profile for users who don't know their economic parameters.
#[derive(Debug, Clone, Copy)]
pub enum Preset {
    Ecommerce,
    Media,
    Api,
}

impl Preset {
    pub fn to_config(self, capacity_bytes: u64) -> ScenarioConfig {
        match self {
            Self::Ecommerce => ScenarioConfig {
                capacity_bytes,
                time_window_seconds: 86400,
                latency_value_per_ms: 0.00005,
                freshness_model: FreshnessModel::TtlOnly {
                    stale_penalty: StalePenaltyConfig {
                        default_class: StalePenaltyClass::High,
                        cost_overrides: StaleCostOverrides::default(),
                    },
                },
                scoring_version: ScoringVersion::default(),
            },
            Self::Media => ScenarioConfig {
                capacity_bytes,
                time_window_seconds: 86400,
                latency_value_per_ms: 0.00001,
                freshness_model: FreshnessModel::TtlOnly {
                    stale_penalty: StalePenaltyConfig {
                        default_class: StalePenaltyClass::Low,
                        cost_overrides: StaleCostOverrides::default(),
                    },
                },
                scoring_version: ScoringVersion::default(),
            },
            Self::Api => ScenarioConfig {
                capacity_bytes,
                time_window_seconds: 86400,
                latency_value_per_ms: 0.0001,
                freshness_model: FreshnessModel::InvalidationOnUpdate {
                    invalidation_cost: 0.001,
                },
                scoring_version: ScoringVersion::default(),
            },
        }
    }
}
