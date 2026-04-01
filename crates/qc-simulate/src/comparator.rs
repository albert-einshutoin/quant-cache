use serde::Serialize;

use qc_model::metrics::MetricsSummary;
use qc_model::trace::RequestTraceEvent;

use crate::engine::{CachePolicy, ReplayEconConfig, TraceReplayEngine};
use crate::error::SimulateError;

/// Result of replaying one policy against a trace.
#[derive(Debug, Clone, Serialize)]
pub struct PolicyResult {
    pub name: String,
    pub metrics: MetricsSummary,
}

/// Result of comparing multiple policies on the same trace.
#[derive(Debug, Clone)]
pub struct ComparisonReport {
    pub results: Vec<PolicyResult>,
}

impl ComparisonReport {
    /// Find the policy with the highest hit ratio.
    pub fn best_by_hit_ratio(&self) -> Option<&PolicyResult> {
        self.results.iter().max_by(|a, b| {
            a.metrics
                .hit_ratio
                .partial_cmp(&b.metrics.hit_ratio)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Find the policy with the highest estimated cost savings.
    pub fn best_by_cost_savings(&self) -> Option<&PolicyResult> {
        self.results.iter().max_by(|a, b| {
            a.metrics
                .estimated_cost_savings
                .partial_cmp(&b.metrics.estimated_cost_savings)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Find the policy with the highest policy_objective_value.
    pub fn best_by_objective(&self) -> Option<&PolicyResult> {
        self.results.iter().max_by(|a, b| {
            a.metrics
                .policy_objective_value
                .partial_cmp(&b.metrics.policy_objective_value)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}

/// Compare multiple cache policies on the same trace.
pub struct Comparator;

impl Comparator {
    pub fn compare(
        events: &[RequestTraceEvent],
        policies: &mut [&mut dyn CachePolicy],
    ) -> Result<ComparisonReport, SimulateError> {
        Self::compare_with_econ(events, policies, &ReplayEconConfig::default())
    }

    pub fn compare_with_econ(
        events: &[RequestTraceEvent],
        policies: &mut [&mut dyn CachePolicy],
        econ: &ReplayEconConfig,
    ) -> Result<ComparisonReport, SimulateError> {
        let mut results = Vec::with_capacity(policies.len());

        for policy in policies.iter_mut() {
            let name = policy.name().to_string();
            let metrics = TraceReplayEngine::replay_with_econ(events, *policy, econ)?;
            results.push(PolicyResult { name, metrics });
        }

        Ok(ComparisonReport { results })
    }
}
