use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::Path;

use qc_model::origin_cost::OriginCostConfig;
use qc_model::trace::{CacheStatus, RequestTraceEvent};

use super::ProviderLogParser;

/// Parser for Fastly real-time log streaming (NDJSON format).
///
/// Fastly logging is configurable; this parser expects NDJSON with fields:
/// url, status, resp_body_bytes, cache_status, time_elapsed, content_type, timestamp
///
/// Configure Fastly logging endpoint with this format:
/// ```json
/// {"timestamp":"%{begin:%s}t","url":"%r","status":%>s,"resp_body_bytes":%B,
///  "cache_status":"%{Fastly-Debug-Digest}i","time_elapsed":%D,"content_type":"%{Content-Type}o"}
/// ```
pub struct FastlyParser;

impl ProviderLogParser for FastlyParser {
    fn name(&self) -> &str {
        "fastly"
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

        loop {
            buf.clear();
            let bytes_read = (&mut reader)
                .take((MAX_LINE_BYTES + 1) as u64)
                .read_until(b'\n', &mut buf)?;
            if bytes_read == 0 {
                break;
            }
            if buf.len() > MAX_LINE_BYTES && !buf.ends_with(b"\n") {
                tracing::warn!("skipping oversized line (>{MAX_LINE_BYTES} bytes)");
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
                Err(_) => continue,
            };
            if line.is_empty() {
                continue;
            }

            match parse_fastly_line(line, cost_config) {
                Ok(Some(e)) => events.push(e),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("skipping malformed line: {e}");
                }
            }
        }

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

fn parse_fastly_line(
    line: &str,
    cost_config: &OriginCostConfig,
) -> anyhow::Result<Option<RequestTraceEvent>> {
    let v: serde_json::Value = serde_json::from_str(line)?;

    // Timestamp: Unix epoch seconds (integer or string)
    let timestamp = if let Some(ts) = v["timestamp"].as_i64() {
        chrono::DateTime::from_timestamp(ts, 0)
            .ok_or_else(|| anyhow::anyhow!("invalid timestamp: {ts}"))?
    } else if let Some(ts_str) = v["timestamp"].as_str() {
        if let Ok(epoch) = ts_str.parse::<i64>() {
            chrono::DateTime::from_timestamp(epoch, 0)
                .ok_or_else(|| anyhow::anyhow!("invalid timestamp: {ts_str}"))?
        } else {
            ts_str.parse::<chrono::DateTime<chrono::Utc>>()?
        }
    } else {
        return Ok(None);
    };

    // URL: may be full request line "GET /path HTTP/1.1" or just "/path"
    let raw_url = v["url"].as_str().unwrap_or("/");
    let uri = if raw_url.starts_with("GET ") || raw_url.starts_with("POST ") {
        raw_url.split_whitespace().nth(1).unwrap_or("/").to_string()
    } else {
        raw_url.to_string()
    };

    let response_bytes = v["resp_body_bytes"]
        .as_u64()
        .or_else(|| v["bytes"].as_u64())
        .unwrap_or(0);
    let status_code = v["status"].as_u64().unwrap_or(0) as u16;
    let content_type = v["content_type"]
        .as_str()
        .or_else(|| v["Content-Type"].as_str())
        .map(|s| s.to_string());

    let cache_status_str = v["cache_status"]
        .as_str()
        .or_else(|| v["fastly_info"].as_str())
        .unwrap_or("");
    let cache_status = match cache_status_str.to_lowercase().as_str() {
        s if s.contains("hit") => Some(CacheStatus::Hit),
        s if s.contains("miss") => Some(CacheStatus::Miss),
        s if s.contains("pass") || s.contains("synth") => Some(CacheStatus::Bypass),
        _ => Some(CacheStatus::Miss),
    };

    let eligible = status_code != 206 && (200..400).contains(&status_code);

    // time_elapsed: microseconds (Fastly %D) → milliseconds
    let time_elapsed_us = v["time_elapsed"]
        .as_f64()
        .or_else(|| v["time_elapsed"].as_i64().map(|i| i as f64))
        .unwrap_or(0.0);
    let latency_ms = time_elapsed_us / 1000.0;
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
