//! Compact trace event representation with interned string IDs.
//!
//! `CompactTraceEvent` replaces per-event `String` heap allocations with `u32`
//! IDs from a shared [`StringInterner`]. This reduces per-event memory from
//! ~200+ bytes to ~64 bytes, enabling 1B+ event processing.

use chrono::{DateTime, Utc};

use crate::intern::StringInterner;
use crate::trace::RequestTraceEvent;

/// Compact representation of a trace event with interned string IDs.
///
/// All string fields (cache_key, object_id, content_type, region,
/// version_or_etag) are stored as `u32` IDs into a shared `StringInterner`.
/// Optional fields use `NONE_ID` (0) as the None sentinel.
#[derive(Debug, Clone, Copy)]
pub struct CompactTraceEvent {
    pub timestamp: DateTime<Utc>,
    pub cache_key_id: u32,
    pub object_id_id: u32,
    pub object_size_bytes: u64,
    pub response_bytes: u64, // 0 means "use object_size_bytes"
    pub origin_fetch_cost: f64,
    pub response_latency_ms: f64,
    pub status_code: u16,
    pub content_type_id: u32,
    pub version_or_etag_id: u32,
    pub region_id: u32,
    pub eligible_for_cache: bool,
    pub has_response_bytes: bool,
}

impl CompactTraceEvent {
    /// Convert a batch of `RequestTraceEvent`s to compact form with interning.
    ///
    /// Returns the compact events and the interner for resolving IDs back to strings.
    pub fn intern_batch(events: &[RequestTraceEvent]) -> (Vec<CompactTraceEvent>, StringInterner) {
        let mut interner = StringInterner::new();
        let compact: Vec<CompactTraceEvent> = events
            .iter()
            .map(|e| Self::from_request(e, &mut interner))
            .collect();
        (compact, interner)
    }

    /// Convert a single event to compact form.
    pub fn from_request(event: &RequestTraceEvent, interner: &mut StringInterner) -> Self {
        Self {
            timestamp: event.timestamp,
            cache_key_id: interner.intern(&event.cache_key),
            object_id_id: interner.intern(&event.object_id),
            object_size_bytes: event.object_size_bytes,
            response_bytes: event.response_bytes.unwrap_or(0),
            has_response_bytes: event.response_bytes.is_some(),
            origin_fetch_cost: event.origin_fetch_cost.unwrap_or(0.0),
            response_latency_ms: event.response_latency_ms.unwrap_or(0.0),
            status_code: event.status_code.unwrap_or(0),
            content_type_id: interner.intern_option(event.content_type.as_deref()),
            version_or_etag_id: interner.intern_option(event.version_or_etag.as_deref()),
            region_id: interner.intern_option(event.region.as_deref()),
            eligible_for_cache: event.eligible_for_cache,
        }
    }

    /// Expand back to a full `RequestTraceEvent` for backward compatibility.
    pub fn to_request_trace_event(&self, interner: &StringInterner) -> RequestTraceEvent {
        RequestTraceEvent {
            schema_version: RequestTraceEvent::SCHEMA_VERSION.to_string(),
            timestamp: self.timestamp,
            object_id: interner.resolve(self.object_id_id).to_string(),
            cache_key: interner.resolve(self.cache_key_id).to_string(),
            object_size_bytes: self.object_size_bytes,
            response_bytes: if self.has_response_bytes {
                Some(self.response_bytes)
            } else {
                None
            },
            cache_status: None, // not stored in compact form
            status_code: if self.status_code > 0 {
                Some(self.status_code)
            } else {
                None
            },
            origin_fetch_cost: Some(self.origin_fetch_cost),
            response_latency_ms: Some(self.response_latency_ms),
            region: interner
                .resolve_option(self.region_id)
                .map(|s| s.to_string()),
            content_type: interner
                .resolve_option(self.content_type_id)
                .map(|s| s.to_string()),
            version_or_etag: interner
                .resolve_option(self.version_or_etag_id)
                .map(|s| s.to_string()),
            eligible_for_cache: self.eligible_for_cache,
        }
    }

    /// Get effective response bytes (response_bytes if present, else object_size).
    pub fn effective_response_bytes(&self) -> u64 {
        if self.has_response_bytes {
            self.response_bytes
        } else {
            self.object_size_bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intern::NONE_ID;
    use crate::trace::{CacheStatus, RequestTraceEvent};

    fn sample_event() -> RequestTraceEvent {
        RequestTraceEvent {
            schema_version: "1.0".into(),
            timestamp: DateTime::from_timestamp(1_000_000, 0).unwrap(),
            object_id: "obj-001".into(),
            cache_key: "/content/001".into(),
            object_size_bytes: 1024,
            response_bytes: Some(1024),
            cache_status: Some(CacheStatus::Hit),
            status_code: Some(200),
            origin_fetch_cost: Some(0.003),
            response_latency_ms: Some(50.0),
            region: Some("us-east-1".into()),
            content_type: Some("image/png".into()),
            version_or_etag: Some("v1".into()),
            eligible_for_cache: true,
        }
    }

    #[test]
    fn round_trip_preserves_fields() {
        let event = sample_event();
        let mut interner = StringInterner::new();
        let compact = CompactTraceEvent::from_request(&event, &mut interner);
        let restored = compact.to_request_trace_event(&interner);

        assert_eq!(restored.cache_key, event.cache_key);
        assert_eq!(restored.object_id, event.object_id);
        assert_eq!(restored.object_size_bytes, event.object_size_bytes);
        assert_eq!(restored.response_bytes, event.response_bytes);
        assert_eq!(restored.status_code, event.status_code);
        assert_eq!(restored.origin_fetch_cost, event.origin_fetch_cost);
        assert_eq!(restored.response_latency_ms, event.response_latency_ms);
        assert_eq!(restored.content_type, event.content_type);
        assert_eq!(restored.version_or_etag, event.version_or_etag);
        assert_eq!(restored.region, event.region);
        assert_eq!(restored.eligible_for_cache, event.eligible_for_cache);
        assert_eq!(restored.timestamp, event.timestamp);
    }

    #[test]
    fn intern_batch_deduplicates() {
        let events = vec![
            sample_event(),
            {
                let mut e = sample_event();
                e.cache_key = "/content/001".into(); // same key
                e
            },
            {
                let mut e = sample_event();
                e.cache_key = "/content/002".into(); // different key
                e
            },
        ];

        let (compact, interner) = CompactTraceEvent::intern_batch(&events);
        assert_eq!(compact.len(), 3);
        assert_eq!(compact[0].cache_key_id, compact[1].cache_key_id); // same key → same ID
        assert_ne!(compact[0].cache_key_id, compact[2].cache_key_id); // different key

        // Interner should have: sentinel + obj-001 + /content/001 + v1 + us-east-1 + image/png + /content/002
        assert!(interner.len() <= 8);
    }

    #[test]
    fn none_fields_use_sentinel() {
        let mut event = sample_event();
        event.region = None;
        event.content_type = None;
        event.version_or_etag = None;

        let mut interner = StringInterner::new();
        let compact = CompactTraceEvent::from_request(&event, &mut interner);

        assert_eq!(compact.region_id, NONE_ID);
        assert_eq!(compact.content_type_id, NONE_ID);
        assert_eq!(compact.version_or_etag_id, NONE_ID);

        let restored = compact.to_request_trace_event(&interner);
        assert_eq!(restored.region, None);
        assert_eq!(restored.content_type, None);
        assert_eq!(restored.version_or_etag, None);
    }

    #[test]
    fn effective_response_bytes() {
        let mut event = sample_event();
        event.response_bytes = Some(2048);
        let mut interner = StringInterner::new();
        let compact = CompactTraceEvent::from_request(&event, &mut interner);
        assert_eq!(compact.effective_response_bytes(), 2048);

        event.response_bytes = None;
        let compact = CompactTraceEvent::from_request(&event, &mut interner);
        assert_eq!(compact.effective_response_bytes(), 1024); // falls back to object_size
    }

    #[test]
    fn memory_size_is_small() {
        assert!(
            std::mem::size_of::<CompactTraceEvent>() <= 80,
            "CompactTraceEvent should be <= 80 bytes, got {}",
            std::mem::size_of::<CompactTraceEvent>()
        );
    }
}
