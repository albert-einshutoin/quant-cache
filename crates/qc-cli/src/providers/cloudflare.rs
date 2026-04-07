use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::Path;

use qc_model::origin_cost::OriginCostConfig;
use qc_model::trace::{CacheStatus, RequestTraceEvent};

use super::ProviderLogParser;

/// Parser for Cloudflare Enterprise Log Share (ELS) / Logpush JSON format.
///
/// Each line is a JSON object with fields like:
/// ClientRequestURI, ClientRequestHost, EdgeResponseBytes,
/// CacheStatus, EdgeStartTimestamp, OriginResponseTime, etc.
pub struct CloudflareParser;

impl ProviderLogParser for CloudflareParser {
    fn name(&self) -> &str {
        "cloudflare"
    }

    fn parse(
        &self,
        path: &Path,
        cost_config: &OriginCostConfig,
    ) -> anyhow::Result<Vec<RequestTraceEvent>> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);

        let mut events = Vec::new();

        const MAX_LINE_BYTES: usize = 64 * 1024;
        let mut buf = Vec::with_capacity(4096);
        let mut reader = reader;
        let mut line_num = 0usize;

        loop {
            buf.clear();
            line_num += 1;
            let bytes_read = (&mut reader)
                .take((MAX_LINE_BYTES + 1) as u64)
                .read_until(b'\n', &mut buf)?;
            if bytes_read == 0 {
                break;
            }
            if buf.len() > MAX_LINE_BYTES && !buf.ends_with(b"\n") {
                tracing::warn!(
                    "{}:{}: skipping oversized line (>{MAX_LINE_BYTES} bytes)",
                    path.display(),
                    line_num
                );
                loop {
                    buf.clear();
                    let n = (&mut reader)
                        .take(MAX_LINE_BYTES as u64)
                        .read_until(b'\n', &mut buf)?;
                    if n == 0 || buf.ends_with(b"\n") {
                        break;
                    }
                }
                continue;
            }
            let line = match std::str::from_utf8(&buf) {
                Ok(s) => s.trim(),
                Err(_) => {
                    tracing::warn!("{}:{}: skipping non-UTF-8 line", path.display(), line_num);
                    continue;
                }
            };
            if line.is_empty() {
                continue;
            }

            match parse_cloudflare_line(line, cost_config) {
                Ok(Some(e)) => events.push(e),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        "{}:{}: skipping malformed line: {e}",
                        path.display(),
                        line_num
                    );
                }
            }
        }

        // Post-process: object_size_bytes as max response_bytes per cache_key
        let max_sizes = compute_max_sizes(&events);
        for event in &mut events {
            if let Some(&max_size) = max_sizes.get(&event.cache_key) {
                event.object_size_bytes = max_size;
            }
        }

        events.sort_by_key(|e| e.timestamp);
        Ok(events)
    }
}

fn parse_cloudflare_line(
    line: &str,
    cost_config: &OriginCostConfig,
) -> anyhow::Result<Option<RequestTraceEvent>> {
    let v: serde_json::Value = serde_json::from_str(line)?;

    // Timestamp: EdgeStartTimestamp (nanoseconds since epoch) or string
    let timestamp = if let Some(ts) = v["EdgeStartTimestamp"].as_i64() {
        // Nanoseconds since epoch
        chrono::DateTime::from_timestamp(ts / 1_000_000_000, (ts % 1_000_000_000) as u32)
            .ok_or_else(|| anyhow::anyhow!("invalid timestamp: {ts}"))?
    } else if let Some(ts_str) = v["EdgeStartTimestamp"].as_str() {
        ts_str.parse::<chrono::DateTime<chrono::Utc>>()?
    } else {
        return Ok(None);
    };

    let uri = v["ClientRequestURI"].as_str().unwrap_or("/").to_string();
    let response_bytes = v["EdgeResponseBytes"].as_u64().unwrap_or(0);
    let status_code = v["EdgeResponseStatus"].as_u64().unwrap_or(0) as u16;
    let content_type = v["EdgeResponseContentType"]
        .as_str()
        .or_else(|| v["ContentType"].as_str())
        .map(|s| s.to_string());

    // Cache status mapping
    let cache_status_str = v["CacheCacheStatus"]
        .as_str()
        .or_else(|| v["CacheStatus"].as_str())
        .unwrap_or("");
    let cache_status = match cache_status_str {
        "hit" | "stale" | "revalidated" => Some(CacheStatus::Hit),
        "miss" | "expired" => Some(CacheStatus::Miss),
        "bypass" | "dynamic" => Some(CacheStatus::Bypass),
        _ => Some(CacheStatus::Miss),
    };

    let eligible = status_code != 206 && (200..400).contains(&status_code);

    // OriginResponseTime: nanoseconds in Cloudflare Logpush v2 → convert to ms
    let origin_time_ns = v["OriginResponseTime"]
        .as_u64()
        .or_else(|| v["OriginResponseTime"].as_f64().map(|f| f as u64))
        .unwrap_or(0);
    let latency_ms = origin_time_ns as f64 / 1_000_000.0;
    let latency_ms = if latency_ms.is_finite() && (0.0..=3600000.0).contains(&latency_ms) {
        latency_ms
    } else {
        0.0
    };

    let origin_cost = cost_config.estimate(
        &uri,
        content_type.as_deref(),
        if latency_ms > 0.0 {
            Some(latency_ms)
        } else {
            None
        },
    );

    Ok(Some(RequestTraceEvent {
        schema_version: RequestTraceEvent::SCHEMA_VERSION.to_string(),
        timestamp,
        object_id: uri.clone(),
        cache_key: uri,
        object_size_bytes: response_bytes,
        response_bytes: Some(response_bytes),
        cache_status,
        status_code: Some(status_code),
        origin_fetch_cost: Some(origin_cost),
        response_latency_ms: Some(latency_ms),
        region: None,
        content_type,
        version_or_etag: None,
        eligible_for_cache: eligible,
    }))
}

fn compute_max_sizes(events: &[RequestTraceEvent]) -> HashMap<String, u64> {
    let mut max_sizes: HashMap<String, u64> = HashMap::new();
    for event in events {
        let size = event.response_bytes.unwrap_or(event.object_size_bytes);
        let entry = max_sizes.entry(event.cache_key.clone()).or_insert(0);
        if size > *entry {
            *entry = size;
        }
    }
    max_sizes
}
