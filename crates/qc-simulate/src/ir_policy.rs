use std::collections::HashMap;

use chrono::{DateTime, Utc};
use qc_model::object::{ObjectFeatures, ScoredObject};
use qc_model::policy_ir::{AdmissionRule, Backend, BypassRule, PolicyIR};
use qc_model::trace::RequestTraceEvent;

use crate::baselines::{S3FifoPolicy, SievePolicy};
use crate::engine::{CacheOutcome, CachePolicy};

/// Runtime context for evaluating a PolicyIR against a trace.
/// Contains pre-computed economic scores and object features.
pub struct IrEvalContext {
    /// cache_key → net_benefit score
    pub scores: HashMap<String, f64>,
    /// cache_key → object size in bytes
    pub sizes: HashMap<String, u64>,
    /// cache_key → freshness risk P(stale) = 1 - exp(-update_rate * ttl)
    pub freshness_risks: HashMap<String, f64>,
}

impl IrEvalContext {
    /// Build from ObjectFeatures and ScoredObjects.
    /// Does not depend on qc-solver — takes pre-computed data.
    pub fn from_features_and_scores(features: &[ObjectFeatures], scored: &[ScoredObject]) -> Self {
        let scores: HashMap<String, f64> = scored
            .iter()
            .map(|s| (s.cache_key.clone(), s.net_benefit))
            .collect();

        let sizes: HashMap<String, u64> = features
            .iter()
            .map(|f| (f.cache_key.clone(), f.size_bytes))
            .collect();

        let freshness_risks: HashMap<String, f64> = features
            .iter()
            .map(|f| {
                let p_stale = 1.0 - (-f.update_rate * f.ttl_seconds as f64).exp();
                (f.cache_key.clone(), p_stale)
            })
            .collect();

        Self {
            scores,
            sizes,
            freshness_risks,
        }
    }
}

/// Internal backend enum (avoids trait objects).
enum BackendInner {
    Sieve(SievePolicy),
    S3Fifo(S3FifoPolicy),
}

impl BackendInner {
    fn new(backend: Backend, capacity_bytes: u64) -> Self {
        match backend {
            Backend::Sieve => Self::Sieve(SievePolicy::new(capacity_bytes)),
            Backend::S3Fifo => Self::S3Fifo(S3FifoPolicy::new(capacity_bytes)),
        }
    }

    fn contains(&self, key: &str) -> bool {
        match self {
            Self::Sieve(p) => p.index.contains_key(key),
            Self::S3Fifo(p) => p.in_small.contains_key(key) || p.in_main.contains_key(key),
        }
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        match self {
            Self::Sieve(p) => CachePolicy::on_request(p, event),
            Self::S3Fifo(p) => CachePolicy::on_request(p, event),
        }
    }
}

/// A CachePolicy built from a PolicyIR + evaluation context.
pub struct IrPolicy {
    ir: PolicyIR,
    context: IrEvalContext,
    backend: BackendInner,
    display_name: String,
    /// Per-object TTL based on ttl_class_rules (cache_key → ttl_seconds).
    ttl_overrides: HashMap<String, u64>,
    /// Track insert times for TTL-based stale detection at IR level.
    insert_times: HashMap<String, DateTime<Utc>>,
}

impl IrPolicy {
    pub fn new(ir: PolicyIR, context: IrEvalContext) -> Self {
        let backend = BackendInner::new(ir.backend, ir.capacity_bytes);
        let display_name = Self::build_name(&ir);
        Self {
            ir,
            context,
            backend,
            display_name,
            ttl_overrides: HashMap::new(),
            insert_times: HashMap::new(),
        }
    }

    /// Resolve TTL for a given content_type using ttl_class_rules.
    /// Returns the override TTL if matched, or the default backend TTL (3600s).
    fn resolve_ttl(&self, content_type: Option<&str>) -> u64 {
        if let Some(ct) = content_type {
            for rule in &self.ir.ttl_class_rules {
                if ct.starts_with(&rule.content_type_pattern) {
                    return rule.ttl_seconds;
                }
            }
        }
        3600 // default backend TTL
    }

    /// Build per-object TTL map from features + ttl_class_rules.
    pub fn apply_ttl_rules(&mut self, events: &[RequestTraceEvent]) {
        if self.ir.ttl_class_rules.is_empty() {
            return;
        }
        for event in events {
            if !self.ttl_overrides.contains_key(&event.cache_key) {
                let ttl = self.resolve_ttl(event.content_type.as_deref());
                self.ttl_overrides.insert(event.cache_key.clone(), ttl);
            }
        }
    }

    fn get_ttl(&self, cache_key: &str) -> u64 {
        self.ttl_overrides.get(cache_key).copied().unwrap_or(3600)
    }

    fn build_name(ir: &PolicyIR) -> String {
        let backend = match ir.backend {
            Backend::Sieve => "SIEVE",
            Backend::S3Fifo => "S3FIFO",
        };
        let admission = match &ir.admission_rule {
            AdmissionRule::Always => "".to_string(),
            AdmissionRule::ScoreThreshold { threshold } => format!("+score>{threshold:.4}"),
            AdmissionRule::ScoreDensityThreshold { threshold } => {
                format!("+density>{threshold:.6}")
            }
        };
        let bypass = match &ir.bypass_rule {
            BypassRule::None => "".to_string(),
            _ => "+bypass".to_string(),
        };
        let prewarm = if ir.prewarm_set.is_empty() {
            "".to_string()
        } else {
            format!("+pw{}", ir.prewarm_set.len())
        };
        format!("IR({backend}{admission}{bypass}{prewarm})")
    }

    /// Pre-warm objects before replay.
    /// Creates synthetic events to insert prewarm objects into the backend.
    /// `trace_start` should be the timestamp of the first real trace event
    /// to avoid immediate stale detection due to TTL expiry.
    pub fn prewarm(&mut self, features: &[ObjectFeatures], trace_start: DateTime<Utc>) {
        let feature_map: HashMap<&str, &ObjectFeatures> =
            features.iter().map(|f| (f.cache_key.as_str(), f)).collect();

        // Insert prewarm objects just before trace starts
        let prewarm_time = trace_start - chrono::Duration::seconds(1);

        for key in &self.ir.prewarm_set {
            if let Some(feat) = feature_map.get(key.as_str()) {
                let event = RequestTraceEvent {
                    schema_version: "1.0".to_string(),
                    timestamp: prewarm_time,
                    object_id: feat.object_id.clone(),
                    cache_key: key.clone(),
                    object_size_bytes: feat.size_bytes,
                    response_bytes: Some(feat.size_bytes),
                    cache_status: None,
                    status_code: Some(200),
                    origin_fetch_cost: Some(feat.avg_origin_cost),
                    response_latency_ms: Some(feat.avg_latency_saving_ms),
                    region: None,
                    content_type: None,
                    version_or_etag: None,
                    eligible_for_cache: true,
                };
                // Insert into backend (this will be a miss → insert)
                self.backend.on_request(&event);
            }
        }
    }

    fn should_bypass(&self, event: &RequestTraceEvent) -> bool {
        Self::eval_bypass(&self.ir.bypass_rule, event, &self.context)
    }

    fn eval_bypass(rule: &BypassRule, event: &RequestTraceEvent, context: &IrEvalContext) -> bool {
        match rule {
            BypassRule::None => false,
            BypassRule::SizeLimit { max_bytes } => event.object_size_bytes > *max_bytes,
            BypassRule::FreshnessRisk { threshold } => context
                .freshness_risks
                .get(&event.cache_key)
                .is_some_and(|&risk| risk > *threshold),
            BypassRule::Any { rules } => rules.iter().any(|r| Self::eval_bypass(r, event, context)),
        }
    }

    fn should_admit(&self, event: &RequestTraceEvent) -> bool {
        match &self.ir.admission_rule {
            AdmissionRule::Always => true,
            AdmissionRule::ScoreThreshold { threshold } => self
                .context
                .scores
                .get(&event.cache_key)
                .is_some_and(|&s| s > *threshold),
            AdmissionRule::ScoreDensityThreshold { threshold } => {
                let score = self
                    .context
                    .scores
                    .get(&event.cache_key)
                    .copied()
                    .unwrap_or(0.0);
                let size = self
                    .context
                    .sizes
                    .get(&event.cache_key)
                    .copied()
                    .unwrap_or(1) as f64;
                (score / size) > *threshold
            }
        }
    }
}

impl CachePolicy for IrPolicy {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        // 1. Bypass rule
        if self.should_bypass(event) {
            return CacheOutcome::Bypass;
        }

        // 2. Already in cache → check TTL stale at IR level, then delegate
        if self.backend.contains(&event.cache_key) {
            let outcome = self.backend.on_request(event);

            // Apply IR-level TTL class stale check
            if outcome == CacheOutcome::Hit {
                if let Some(&insert_time) = self.insert_times.get(&event.cache_key) {
                    let ttl = self.get_ttl(&event.cache_key);
                    let age = (event.timestamp - insert_time).num_seconds();
                    if age > ttl as i64 {
                        // Refresh insert time on stale
                        self.insert_times
                            .insert(event.cache_key.clone(), event.timestamp);
                        return CacheOutcome::StaleHit;
                    }
                }
            }
            // Track/refresh insert time on hit
            self.insert_times
                .entry(event.cache_key.clone())
                .or_insert(event.timestamp);
            return outcome;
        }

        // 3. Admission check
        if !self.should_admit(event) {
            return CacheOutcome::Miss;
        }

        // 4. Admitted → delegate to backend (insert + return Miss)
        let outcome = self.backend.on_request(event);
        // Track insert time for new objects
        self.insert_times
            .insert(event.cache_key.clone(), event.timestamp);
        outcome
    }
}
