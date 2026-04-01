use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CacheStatus {
    Hit,
    Miss,
    Expired,
    Bypass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestTraceEvent {
    pub schema_version: String,
    pub timestamp: DateTime<Utc>,
    pub object_id: String,
    pub cache_key: String,
    pub object_size_bytes: u64,
    pub response_bytes: Option<u64>,
    pub cache_status: Option<CacheStatus>,
    pub status_code: Option<u16>,
    pub origin_fetch_cost: Option<f64>,
    pub response_latency_ms: Option<f64>,
    pub region: Option<String>,
    pub content_type: Option<String>,
    pub version_or_etag: Option<String>,
    pub eligible_for_cache: bool,
}

impl RequestTraceEvent {
    pub const SCHEMA_VERSION: &'static str = "1.0";
}
