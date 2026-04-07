pub mod cloudflare;
pub mod cloudfront;
pub mod fastly;

use qc_model::origin_cost::OriginCostConfig;
use qc_model::trace::RequestTraceEvent;

/// Trait for parsing provider-specific log formats into canonical trace events.
pub trait ProviderLogParser {
    fn name(&self) -> &str;

    /// Parse a log file into a list of trace events.
    fn parse(
        &self,
        path: &std::path::Path,
        cost_config: &OriginCostConfig,
    ) -> anyhow::Result<Vec<RequestTraceEvent>>;
}
