use serde::{Deserialize, Serialize};

/// Configuration for estimating origin fetch cost from CDN logs.
///
/// Fallback chain: explicit rule → content-type default → latency-derived → global default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginCostConfig {
    /// Explicit rules matching path prefix or content-type to a fixed cost.
    #[serde(default)]
    pub rules: Vec<CostRule>,
    /// Default cost by content-type (e.g., "image/jpeg" → 0.001).
    #[serde(default)]
    pub content_type_defaults: std::collections::HashMap<String, f64>,
    /// Derive cost from origin latency: cost = latency_ms * this rate.
    pub latency_cost_per_ms: Option<f64>,
    /// Final fallback cost ($/request).
    #[serde(default = "default_global_cost")]
    pub global_default: f64,
}

fn default_global_cost() -> f64 {
    0.003
}

impl Default for OriginCostConfig {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            content_type_defaults: std::collections::HashMap::new(),
            latency_cost_per_ms: None,
            global_default: 0.003,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostRule {
    /// Match by path prefix (e.g., "/api/").
    pub path_prefix: Option<String>,
    /// Match by content-type (e.g., "application/json").
    pub content_type: Option<String>,
    /// Cost to assign ($/request).
    pub cost: f64,
}

impl OriginCostConfig {
    /// Estimate origin cost for a request using the fallback chain.
    pub fn estimate(&self, path: &str, content_type: Option<&str>, latency_ms: Option<f64>) -> f64 {
        // 1. Explicit rules
        for rule in &self.rules {
            if let Some(ref prefix) = rule.path_prefix {
                if path.starts_with(prefix) {
                    return rule.cost;
                }
            }
            if let (Some(ref ct_rule), Some(ct)) = (&rule.content_type, content_type) {
                if ct == ct_rule {
                    return rule.cost;
                }
            }
        }

        // 2. Content-type defaults
        if let Some(ct) = content_type {
            if let Some(&cost) = self.content_type_defaults.get(ct) {
                return cost;
            }
        }

        // 3. Latency-derived
        if let (Some(rate), Some(ms)) = (self.latency_cost_per_ms, latency_ms) {
            return ms * rate;
        }

        // 4. Global default
        self.global_default
    }
}
