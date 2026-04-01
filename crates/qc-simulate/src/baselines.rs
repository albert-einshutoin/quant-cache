use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use qc_model::trace::RequestTraceEvent;

use crate::engine::{CacheOutcome, CachePolicy};

// ── Stale detection helper ──────────────────────────────────────────

/// Check if a cached item is stale by TTL expiry or version mismatch.
fn check_stale(
    insert_time: DateTime<Utc>,
    cached_version: Option<&str>,
    event: &RequestTraceEvent,
    ttl_seconds: u64,
) -> bool {
    let age = (event.timestamp - insert_time).num_seconds();
    if age > ttl_seconds as i64 {
        return true;
    }
    if let (Some(cached_ver), Some(ref event_ver)) = (cached_version, &event.version_or_etag) {
        if cached_ver != event_ver.as_str() {
            return true;
        }
    }
    false
}

// ── Static Policy (from solver output) ──────────────────────────────

/// A static cache policy: a fixed set of cache keys to cache.
/// Tracks insertion time and version for stale detection.
pub struct StaticPolicy {
    cached_keys: std::collections::HashSet<String>,
    name: String,
    ttl_seconds: u64,
    insert_times: HashMap<String, DateTime<Utc>>,
    insert_versions: HashMap<String, Option<String>>,
}

impl StaticPolicy {
    pub fn new(cached_keys: impl IntoIterator<Item = String>) -> Self {
        Self {
            cached_keys: cached_keys.into_iter().collect(),
            name: "EconomicGreedy".to_string(),
            ttl_seconds: 3600,
            insert_times: HashMap::new(),
            insert_versions: HashMap::new(),
        }
    }

    pub fn new_with_name(cached_keys: impl IntoIterator<Item = String>, name: &str) -> Self {
        Self {
            cached_keys: cached_keys.into_iter().collect(),
            name: name.to_string(),
            ttl_seconds: 3600,
            insert_times: HashMap::new(),
            insert_versions: HashMap::new(),
        }
    }

    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }
}

impl CachePolicy for StaticPolicy {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }
        if !self.cached_keys.contains(&event.cache_key) {
            return CacheOutcome::Miss;
        }

        let stale = if let Some(&insert_time) = self.insert_times.get(&event.cache_key) {
            let cached_ver = self
                .insert_versions
                .get(&event.cache_key)
                .and_then(|v| v.as_deref());
            check_stale(insert_time, cached_ver, event, self.ttl_seconds)
        } else {
            false // first access, not stale
        };

        // Refresh on access
        self.insert_times
            .insert(event.cache_key.clone(), event.timestamp);
        self.insert_versions
            .insert(event.cache_key.clone(), event.version_or_etag.clone());

        if stale {
            CacheOutcome::StaleHit
        } else {
            CacheOutcome::Hit
        }
    }
}

// ── LRU Baseline ────────────────────────────────────────────────────

struct LruEntry {
    size: u64,
    insert_time: DateTime<Utc>,
    version: Option<String>,
}

/// Least Recently Used eviction policy with stale detection.
pub struct LruPolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    order: VecDeque<String>,
    entries: HashMap<String, LruEntry>,
}

impl LruPolicy {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            ttl_seconds: 3600,
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    fn promote(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
            self.order.push_back(key.to_string());
        }
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes {
            if let Some(victim) = self.order.pop_front() {
                if let Some(entry) = self.entries.remove(&victim) {
                    self.used_bytes -= entry.size;
                }
            } else {
                break;
            }
        }
    }

    fn insert(&mut self, event: &RequestTraceEvent) {
        let size = event.object_size_bytes;
        if size > self.capacity_bytes {
            return;
        }
        self.evict_until_fits(size);
        self.entries.insert(
            event.cache_key.clone(),
            LruEntry {
                size,
                insert_time: event.timestamp,
                version: event.version_or_etag.clone(),
            },
        );
        self.order.push_back(event.cache_key.clone());
        self.used_bytes += size;
    }
}

impl CachePolicy for LruPolicy {
    fn name(&self) -> &str {
        "LRU"
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        if self.entries.contains_key(&event.cache_key) {
            let stale = {
                let entry = self.entries.get(&event.cache_key).unwrap();
                check_stale(
                    entry.insert_time,
                    entry.version.as_deref(),
                    event,
                    self.ttl_seconds,
                )
            };
            self.promote(&event.cache_key);

            if stale {
                if let Some(entry) = self.entries.get_mut(&event.cache_key) {
                    entry.insert_time = event.timestamp;
                    entry.version = event.version_or_etag.clone();
                }
                CacheOutcome::StaleHit
            } else {
                CacheOutcome::Hit
            }
        } else {
            self.insert(event);
            CacheOutcome::Miss
        }
    }
}

// ── GDSF Baseline ───────────────────────────────────────────────────

struct GdsfEntry {
    size: u64,
    freq: u64,
    cost: f64,
    priority: f64,
    insert_time: DateTime<Utc>,
    version: Option<String>,
}

/// GreedyDual-Size-Frequency eviction policy with stale detection.
pub struct GdsfPolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    inflation: f64,
    entries: HashMap<String, GdsfEntry>,
}

impl GdsfPolicy {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            ttl_seconds: 3600,
            inflation: 0.0,
            entries: HashMap::new(),
        }
    }

    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    fn compute_priority(&self, cost: f64, freq: u64, size: u64) -> f64 {
        self.inflation + cost * freq as f64 / size as f64
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes {
            let victim = self
                .entries
                .iter()
                .min_by(|a, b| {
                    a.1.priority
                        .partial_cmp(&b.1.priority)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(k, v)| (k.clone(), v.priority));

            if let Some((key, priority)) = victim {
                self.inflation = priority;
                if let Some(entry) = self.entries.remove(&key) {
                    self.used_bytes -= entry.size;
                }
            } else {
                break;
            }
        }
    }
}

impl CachePolicy for GdsfPolicy {
    fn name(&self) -> &str {
        "GDSF"
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        let cost = event.origin_fetch_cost.unwrap_or(1.0);

        if self.entries.contains_key(&event.cache_key) {
            let stale = {
                let entry = self.entries.get(&event.cache_key).unwrap();
                check_stale(
                    entry.insert_time,
                    entry.version.as_deref(),
                    event,
                    self.ttl_seconds,
                )
            };

            let entry = self.entries.get_mut(&event.cache_key).unwrap();
            entry.freq += 1;
            entry.priority = self.inflation + entry.cost * entry.freq as f64 / entry.size as f64;

            if stale {
                entry.insert_time = event.timestamp;
                entry.version = event.version_or_etag.clone();
                CacheOutcome::StaleHit
            } else {
                CacheOutcome::Hit
            }
        } else {
            let size = event.object_size_bytes;
            if size > self.capacity_bytes {
                return CacheOutcome::Miss;
            }

            self.evict_until_fits(size);

            let freq = 1;
            let priority = self.compute_priority(cost, freq, size);
            self.entries.insert(
                event.cache_key.clone(),
                GdsfEntry {
                    size,
                    freq,
                    cost,
                    priority,
                    insert_time: event.timestamp,
                    version: event.version_or_etag.clone(),
                },
            );
            self.used_bytes += size;

            CacheOutcome::Miss
        }
    }
}

// ── Belady (MIN) Oracle ─────────────────────────────────────────────

/// Belady's optimal replacement policy (offline oracle).
///
/// Requires the full trace upfront to build a future-access index.
/// On eviction, removes the object whose next access is farthest in the future.
pub struct BeladyPolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    /// cache_key → size, insert_time, version
    entries: HashMap<String, BeladyEntry>,
    /// cache_key → queue of future access positions (indices into the trace)
    future_accesses: HashMap<String, VecDeque<usize>>,
    /// Current position in the trace
    current_pos: usize,
}

struct BeladyEntry {
    size: u64,
    insert_time: DateTime<Utc>,
    version: Option<String>,
}

impl BeladyPolicy {
    /// Build a Belady policy from the full trace.
    /// Must be called before replay.
    pub fn new(events: &[RequestTraceEvent], capacity_bytes: u64) -> Self {
        let mut future_accesses: HashMap<String, VecDeque<usize>> = HashMap::new();
        for (i, event) in events.iter().enumerate() {
            if event.eligible_for_cache {
                future_accesses
                    .entry(event.cache_key.clone())
                    .or_default()
                    .push_back(i);
            }
        }

        Self {
            capacity_bytes,
            used_bytes: 0,
            ttl_seconds: 3600,
            entries: HashMap::new(),
            future_accesses,
            current_pos: 0,
        }
    }

    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    /// Next access position for a cache_key after the current position.
    fn next_access(&self, key: &str) -> usize {
        if let Some(queue) = self.future_accesses.get(key) {
            for &pos in queue {
                if pos > self.current_pos {
                    return pos;
                }
            }
        }
        usize::MAX // never accessed again
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes {
            // Find the cached object whose next access is farthest
            let victim = self
                .entries
                .keys()
                .map(|k| (k.clone(), self.next_access(k)))
                .max_by_key(|(_, next)| *next);

            if let Some((key, _)) = victim {
                if let Some(entry) = self.entries.remove(&key) {
                    self.used_bytes -= entry.size;
                }
            } else {
                break;
            }
        }
    }
}

impl CachePolicy for BeladyPolicy {
    fn name(&self) -> &str {
        "Belady"
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        // Consume the current position from future_accesses
        if let Some(queue) = self.future_accesses.get_mut(&event.cache_key) {
            while queue.front().is_some_and(|&pos| pos <= self.current_pos) {
                queue.pop_front();
            }
        }

        if !event.eligible_for_cache {
            self.current_pos += 1;
            return CacheOutcome::Bypass;
        }

        let result = if self.entries.contains_key(&event.cache_key) {
            let stale = {
                let entry = self.entries.get(&event.cache_key).unwrap();
                check_stale(
                    entry.insert_time,
                    entry.version.as_deref(),
                    event,
                    self.ttl_seconds,
                )
            };
            if stale {
                if let Some(entry) = self.entries.get_mut(&event.cache_key) {
                    entry.insert_time = event.timestamp;
                    entry.version = event.version_or_etag.clone();
                }
                CacheOutcome::StaleHit
            } else {
                CacheOutcome::Hit
            }
        } else {
            let size = event.object_size_bytes;
            if size <= self.capacity_bytes {
                self.evict_until_fits(size);
                self.entries.insert(
                    event.cache_key.clone(),
                    BeladyEntry {
                        size,
                        insert_time: event.timestamp,
                        version: event.version_or_etag.clone(),
                    },
                );
                self.used_bytes += size;
            }
            CacheOutcome::Miss
        };

        self.current_pos += 1;
        result
    }
}
