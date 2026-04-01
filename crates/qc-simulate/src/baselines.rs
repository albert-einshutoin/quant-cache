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

// ── SIEVE (NSDI 2024 Best Paper) ────────────────────────────────────

/// SIEVE eviction policy: FIFO queue + visited bit + hand pointer.
///
/// On hit: set visited=1 (lazy promotion, no queue reorder).
/// On eviction: hand scans for first unvisited object, evicts it.
/// Ref: Zhang et al., "SIEVE is Simpler than LRU", NSDI 2024.
pub struct SievePolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    /// Objects in insertion order. Each entry: (cache_key, size, visited).
    queue: VecDeque<(String, u64, bool)>,
    /// Fast lookup: cache_key → index in queue.
    pub(crate) index: HashMap<String, usize>,
    /// Hand position for eviction scan.
    hand: usize,
}

impl SievePolicy {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            queue: VecDeque::new(),
            index: HashMap::new(),
            hand: 0,
        }
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes && !self.queue.is_empty() {
            // Scan from hand position for an unvisited object
            let len = self.queue.len();
            let mut scanned = 0;
            while scanned < len {
                let pos = self.hand % len;
                if !self.queue[pos].2 {
                    // Evict this unvisited object
                    let (key, size, _) = self.queue.remove(pos).unwrap();
                    self.index.remove(&key);
                    self.used_bytes -= size;
                    // Rebuild indices after removal
                    self.rebuild_index();
                    if self.hand > 0 && pos < self.hand {
                        self.hand -= 1;
                    }
                    self.hand %= self.queue.len().max(1);
                    break;
                } else {
                    // Reset visited bit and advance hand
                    self.queue[pos].2 = false;
                    self.hand = (self.hand + 1) % len;
                }
                scanned += 1;
            }
            if scanned == len {
                // All visited — evict at hand (after resetting all)
                let pos = self.hand % self.queue.len().max(1);
                if let Some((key, size, _)) = self.queue.remove(pos) {
                    self.index.remove(&key);
                    self.used_bytes -= size;
                    self.rebuild_index();
                    self.hand = 0;
                } else {
                    break;
                }
            }
        }
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        for (i, (key, _, _)) in self.queue.iter().enumerate() {
            self.index.insert(key.clone(), i);
        }
    }
}

impl CachePolicy for SievePolicy {
    fn name(&self) -> &str {
        "SIEVE"
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        if let Some(&idx) = self.index.get(&event.cache_key) {
            // Hit: set visited = true (lazy promotion)
            self.queue[idx].2 = true;
            CacheOutcome::Hit
        } else {
            // Miss: insert at tail
            let size = event.object_size_bytes;
            if size > self.capacity_bytes {
                return CacheOutcome::Miss;
            }
            self.evict_until_fits(size);
            let idx = self.queue.len();
            self.queue.push_back((event.cache_key.clone(), size, false));
            self.index.insert(event.cache_key.clone(), idx);
            self.used_bytes += size;
            CacheOutcome::Miss
        }
    }
}

// ── S3-FIFO (SOSP 2023) ────────────────────────────────────────────

/// S3-FIFO: three static FIFO queues with quick demotion.
///
/// Small (10%): probationary. Objects enter here.
/// Main (90%): promoted from Small on hit.
/// Ghost: metadata-only for recently evicted from Small.
/// Ref: Yang et al., "FIFO Queues are All You Need", SOSP 2023.
pub struct S3FifoPolicy {
    small_capacity: u64,
    main_capacity: u64,
    small_used: u64,
    main_used: u64,
    /// Small queue: (cache_key, size, freq_counter)
    small: VecDeque<(String, u64, u8)>,
    /// Main queue: (cache_key, size, freq_counter)
    main: VecDeque<(String, u64, u8)>,
    /// Ghost set: recently evicted from Small (metadata only)
    ghost: std::collections::HashSet<String>,
    ghost_max: usize,
    /// Fast lookup for cache membership
    pub(crate) in_small: HashMap<String, usize>,
    pub(crate) in_main: HashMap<String, usize>,
}

impl S3FifoPolicy {
    pub fn new(capacity_bytes: u64) -> Self {
        let small_capacity = capacity_bytes / 10; // 10%
        let main_capacity = capacity_bytes - small_capacity; // 90%
        Self {
            small_capacity,
            main_capacity,
            small_used: 0,
            main_used: 0,
            small: VecDeque::new(),
            main: VecDeque::new(),
            ghost: std::collections::HashSet::new(),
            ghost_max: 1000,
            in_small: HashMap::new(),
            in_main: HashMap::new(),
        }
    }

    fn evict_small(&mut self) {
        if let Some((key, size, freq)) = self.small.pop_front() {
            self.in_small.remove(&key);
            self.small_used -= size;

            if freq > 0 {
                // Promote to main
                self.insert_main(&key, size);
            } else {
                // Quick demotion: discard (one-hit wonder)
                self.ghost.insert(key);
                if self.ghost.len() > self.ghost_max {
                    if let Some(old) = self.ghost.iter().next().cloned() {
                        self.ghost.remove(&old);
                    }
                }
            }
        }
    }

    fn evict_main(&mut self) {
        while let Some((key, size, freq)) = self.main.pop_front() {
            self.in_main.remove(&key);
            self.main_used -= size;
            if freq > 0 {
                // Re-insert with decremented freq
                self.main.push_back((key.clone(), size, freq - 1));
                let idx = self.main.len() - 1;
                self.in_main.insert(key, idx);
                self.main_used += size;
            } else {
                break;
            }
        }
    }

    fn insert_main(&mut self, key: &str, size: u64) {
        while self.main_used + size > self.main_capacity && !self.main.is_empty() {
            self.evict_main();
        }
        let idx = self.main.len();
        self.main.push_back((key.to_string(), size, 0));
        self.in_main.insert(key.to_string(), idx);
        self.main_used += size;
    }
}

impl CachePolicy for S3FifoPolicy {
    fn name(&self) -> &str {
        "S3-FIFO"
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        // Check if in small queue
        if let Some(&idx) = self.in_small.get(&event.cache_key) {
            if idx < self.small.len() {
                self.small[idx].2 = self.small[idx].2.saturating_add(1).min(3);
            }
            return CacheOutcome::Hit;
        }

        // Check if in main queue
        if let Some(&idx) = self.in_main.get(&event.cache_key) {
            if idx < self.main.len() {
                self.main[idx].2 = self.main[idx].2.saturating_add(1).min(3);
            }
            return CacheOutcome::Hit;
        }

        // Cache miss — insert to small
        let size = event.object_size_bytes;
        if size > self.small_capacity + self.main_capacity {
            return CacheOutcome::Miss;
        }

        while self.small_used + size > self.small_capacity && !self.small.is_empty() {
            self.evict_small();
        }

        let idx = self.small.len();
        let initial_freq = if self.ghost.contains(&event.cache_key) {
            self.ghost.remove(&event.cache_key);
            1 // Ghost hit → start with freq=1 (will be promoted on next eviction)
        } else {
            0
        };

        self.small
            .push_back((event.cache_key.clone(), size, initial_freq));
        self.in_small.insert(event.cache_key.clone(), idx);
        self.small_used += size;

        CacheOutcome::Miss
    }
}

// ── Economic Admission Gate ─────────────────────────────────────────

/// Economic admission gate: only admits objects with positive net benefit.
/// Used in combination with an eviction policy (SIEVE, S3-FIFO).
pub struct EconomicAdmission {
    /// cache_key → net_benefit from economic scoring
    scores: HashMap<String, f64>,
    /// Admission threshold (objects with benefit > threshold are admitted)
    threshold: f64,
}

impl EconomicAdmission {
    pub fn new(scores: HashMap<String, f64>) -> Self {
        Self {
            scores,
            threshold: 0.0,
        }
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    pub fn should_admit(&self, cache_key: &str) -> bool {
        self.scores
            .get(cache_key)
            .is_some_and(|&b| b > self.threshold)
    }
}

// ── EconomicAdmission + SIEVE Hybrid ────────────────────────────────

/// SIEVE eviction with economic admission gate.
/// Only admits objects that pass the economic benefit threshold.
/// Hit handling is identical to pure SIEVE.
pub struct EconSievePolicy {
    admission: EconomicAdmission,
    inner: SievePolicy,
}

impl EconSievePolicy {
    pub fn new(scores: HashMap<String, f64>, capacity_bytes: u64) -> Self {
        Self {
            admission: EconomicAdmission::new(scores),
            inner: SievePolicy::new(capacity_bytes),
        }
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.admission = self.admission.with_threshold(threshold);
        self
    }
}

impl CachePolicy for EconSievePolicy {
    fn name(&self) -> &str {
        "Econ+SIEVE"
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        // If already cached, handle as normal SIEVE hit
        if self.inner.index.contains_key(&event.cache_key) {
            return self.inner.on_request(event);
        }

        // Miss: check admission gate
        if !self.admission.should_admit(&event.cache_key) {
            return CacheOutcome::Miss; // rejected by admission gate
        }

        // Admitted: let SIEVE handle insertion
        self.inner.on_request(event)
    }
}

// ── EconomicAdmission + S3-FIFO Hybrid ──────────────────────────────

/// S3-FIFO eviction with economic admission gate.
pub struct EconS3FifoPolicy {
    admission: EconomicAdmission,
    inner: S3FifoPolicy,
}

impl EconS3FifoPolicy {
    pub fn new(scores: HashMap<String, f64>, capacity_bytes: u64) -> Self {
        Self {
            admission: EconomicAdmission::new(scores),
            inner: S3FifoPolicy::new(capacity_bytes),
        }
    }

    pub fn with_threshold(mut self, threshold: f64) -> Self {
        self.admission = self.admission.with_threshold(threshold);
        self
    }
}

impl CachePolicy for EconS3FifoPolicy {
    fn name(&self) -> &str {
        "Econ+S3FIFO"
    }

    fn on_request(&mut self, event: &RequestTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        // If already in cache, handle normally
        if self.inner.in_small.contains_key(&event.cache_key)
            || self.inner.in_main.contains_key(&event.cache_key)
        {
            return self.inner.on_request(event);
        }

        // Miss: check admission gate
        if !self.admission.should_admit(&event.cache_key) {
            return CacheOutcome::Miss;
        }

        self.inner.on_request(event)
    }
}
