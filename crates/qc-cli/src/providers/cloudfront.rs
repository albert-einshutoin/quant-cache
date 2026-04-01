use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use chrono::NaiveDateTime;
use qc_model::origin_cost::OriginCostConfig;
use qc_model::trace::{CacheStatus, RequestTraceEvent};

use super::ProviderLogParser;

/// Parser for AWS CloudFront standard access logs.
///
/// CloudFront log format (tab-separated, comment lines start with #):
/// date time x-edge-location sc-bytes c-ip cs-method cs(Host) cs-uri-stem
/// sc-status cs(Referer) cs(User-Agent) cs-uri-query cs(Cookie)
/// x-edge-result-type x-edge-request-id x-host-header cs-protocol
/// cs-bytes time-taken x-forwarded-for ssl-protocol ssl-cipher
/// x-edge-response-result-type cs-protocol-version fle-status fle-encrypted-fields
/// c-port time-to-first-byte x-edge-detailed-result-type sc-content-type
/// sc-content-len sc-range-start sc-range-end
pub struct CloudFrontParser;

impl ProviderLogParser for CloudFrontParser {
    fn name(&self) -> &str {
        "cloudfront"
    }

    fn parse(
        &self,
        path: &Path,
        cost_config: &OriginCostConfig,
    ) -> anyhow::Result<Vec<RequestTraceEvent>> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);

        let mut events = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.starts_with('#') || line.is_empty() {
                continue;
            }

            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 26 {
                continue;
            }

            let event = parse_cloudfront_line(&fields, cost_config);
            match event {
                Ok(Some(e)) => events.push(e),
                Ok(None) => {} // skipped (error/redirect)
                Err(e) => {
                    tracing::warn!("skipping malformed line: {e}");
                }
            }
        }

        // Post-process: estimate object_size_bytes as max response_bytes per cache_key
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

fn parse_cloudfront_line(
    fields: &[&str],
    cost_config: &OriginCostConfig,
) -> anyhow::Result<Option<RequestTraceEvent>> {
    // Field indices (0-based) per CloudFront standard log format:
    // 0: date, 1: time, 2: x-edge-location, 3: sc-bytes, 4: c-ip
    // 5: cs-method, 6: cs(Host), 7: cs-uri-stem, 8: sc-status
    // 9: cs(Referer), 10: cs(User-Agent), 11: cs-uri-query, 12: cs(Cookie)
    // 13: x-edge-result-type, 14: x-edge-request-id, 15: x-host-header
    // 16: cs-protocol, 17: cs-bytes, 18: time-taken
    // 29: sc-content-type

    let date_str = fields[0];
    let time_str = fields[1];
    let sc_bytes: u64 = fields[3].parse().unwrap_or(0);
    let uri_stem = fields[7];
    let status_code: u16 = fields[8].parse().unwrap_or(0);
    let uri_query = fields[11];
    let result_type = fields[13];
    let time_taken: f64 = fields[18].parse().unwrap_or(0.0);
    let content_type = if fields.len() > 29 && fields[29] != "-" {
        Some(fields[29].to_string())
    } else {
        None
    };

    // Skip errors and redirects
    match result_type {
        "Error" | "Redirect" | "LimitExceeded" | "CapacityExceeded" => return Ok(None),
        _ => {}
    }

    // Parse timestamp
    let datetime_str = format!("{date_str} {time_str}");
    let naive = NaiveDateTime::parse_from_str(&datetime_str, "%Y-%m-%d %H:%M:%S")
        .map_err(|e| anyhow::anyhow!("timestamp parse error: {e}"))?;
    let timestamp = naive.and_utc();

    // Build cache_key
    let cache_key = if uri_query == "-" || uri_query.is_empty() {
        uri_stem.to_string()
    } else {
        format!("{uri_stem}?{uri_query}")
    };

    // Map result type to CacheStatus
    let cache_status = match result_type {
        "Hit" | "RefreshHit" => Some(CacheStatus::Hit),
        "Miss" => Some(CacheStatus::Miss),
        _ => Some(CacheStatus::Bypass),
    };

    // 206 Partial Content → not eligible for cache by default
    let eligible = status_code != 206 && (200..400).contains(&status_code);

    // Estimate origin cost
    let latency_ms = time_taken * 1000.0; // CloudFront time-taken is in seconds
    let origin_cost = cost_config.estimate(uri_stem, content_type.as_deref(), Some(latency_ms));

    Ok(Some(RequestTraceEvent {
        schema_version: RequestTraceEvent::SCHEMA_VERSION.to_string(),
        timestamp,
        object_id: uri_stem.to_string(),
        cache_key,
        object_size_bytes: sc_bytes, // will be replaced by max per cache_key
        response_bytes: Some(sc_bytes),
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
