use std::collections::{BTreeMap, HashMap, VecDeque};

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
    generation: u64,
    insert_time: DateTime<Utc>,
    version: Option<String>,
}

/// Least Recently Used eviction policy with stale detection.
///
/// Uses a generation counter + BTreeMap for O(1) promote and O(log n) eviction.
pub struct LruPolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    generation: u64,
    entries: HashMap<String, LruEntry>,
    /// generation → cache_key for O(log n) min-generation eviction.
    order: BTreeMap<u64, String>,
}

impl LruPolicy {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            ttl_seconds: 3600,
            generation: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    fn touch(&mut self, key: &str) {
        if let Some(entry) = self.entries.get_mut(key) {
            self.order.remove(&entry.generation);
            self.generation += 1;
            entry.generation = self.generation;
            self.order.insert(self.generation, key.to_string());
        }
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes {
            if let Some((&gen, _)) = self.order.iter().next() {
                let key = self.order.remove(&gen).unwrap();
                if let Some(entry) = self.entries.remove(&key) {
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
        self.generation += 1;
        self.order.insert(self.generation, event.cache_key.clone());
        self.entries.insert(
            event.cache_key.clone(),
            LruEntry {
                size,
                generation: self.generation,
                insert_time: event.timestamp,
                version: event.version_or_etag.clone(),
            },
        );
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

        if let Some(entry) = self.entries.get(&event.cache_key) {
            let stale = check_stale(
                entry.insert_time,
                entry.version.as_deref(),
                event,
                self.ttl_seconds,
            );
            self.touch(&event.cache_key);

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
    /// Unique key in the priority BTreeMap for O(log n) eviction.
    priority_key: (u64, u64),
    insert_time: DateTime<Utc>,
    version: Option<String>,
}

/// GreedyDual-Size-Frequency eviction policy with stale detection.
///
/// Uses a BTreeMap priority index for O(log n) eviction instead of O(n) scan.
pub struct GdsfPolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    inflation: f64,
    entries: HashMap<String, GdsfEntry>,
    /// (priority_bits, seq) → cache_key for O(log n) min-priority eviction.
    priority_index: BTreeMap<(u64, u64), String>,
    /// Monotonic counter for tie-breaking in priority_index.
    seq: u64,
}

impl GdsfPolicy {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            ttl_seconds: 3600,
            inflation: 0.0,
            entries: HashMap::new(),
            priority_index: BTreeMap::new(),
            seq: 0,
        }
    }

    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    fn compute_priority(&self, cost: f64, freq: u64, size: u64) -> f64 {
        if size == 0 {
            return self.inflation;
        }
        self.inflation + cost * freq as f64 / size as f64
    }

    fn priority_bits(p: f64) -> u64 {
        // Clamp NaN/Inf to 0.0 to prevent BTreeMap corruption
        let p = if p.is_finite() { p } else { 0.0 };
        let bits = p.to_bits();
        // Flip so that BTreeMap ordering matches f64 ordering
        if bits & (1u64 << 63) != 0 {
            !bits
        } else {
            bits ^ (1u64 << 63)
        }
    }

    fn next_priority_key(&mut self, priority: f64) -> (u64, u64) {
        self.seq += 1;
        (Self::priority_bits(priority), self.seq)
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes {
            if let Some((&pkey, _)) = self.priority_index.iter().next() {
                let key = self.priority_index.remove(&pkey).unwrap();
                if let Some(entry) = self.entries.remove(&key) {
                    if entry.priority.is_finite() {
                        self.inflation = entry.priority;
                    }
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
            // Copy data needed for stale check and priority update
            let entry = self.entries.get(&event.cache_key).unwrap();
            let stale = check_stale(
                entry.insert_time,
                entry.version.as_deref(),
                event,
                self.ttl_seconds,
            );
            let old_pkey = entry.priority_key;

            // Update priority index
            self.priority_index.remove(&old_pkey);

            let inflation = self.inflation;
            let new_pkey = self.next_priority_key(0.0); // placeholder, update below

            let entry = self.entries.get_mut(&event.cache_key).unwrap();
            entry.freq += 1;
            entry.priority = if entry.size > 0 {
                inflation + entry.cost * entry.freq as f64 / entry.size as f64
            } else {
                inflation
            };
            let pkey = (Self::priority_bits(entry.priority), new_pkey.1);
            entry.priority_key = pkey;
            self.priority_index.insert(pkey, event.cache_key.clone());

            if stale {
                let entry = self.entries.get_mut(&event.cache_key).unwrap();
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
            let pkey = self.next_priority_key(priority);
            self.priority_index.insert(pkey, event.cache_key.clone());
            self.entries.insert(
                event.cache_key.clone(),
                GdsfEntry {
                    size,
                    freq,
                    cost,
                    priority,
                    priority_key: pkey,
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

/// SIEVE entry stored in the circular queue.
struct SieveEntry {
    key: String,
    size: u64,
    visited: bool,
    alive: bool,
    insert_time: DateTime<Utc>,
    version: Option<String>,
}

/// SIEVE eviction policy: FIFO queue + visited bit + hand pointer.
///
/// On hit: set visited=1 (lazy promotion, no queue reorder).
/// On eviction: hand scans for first unvisited object, evicts it.
/// Uses tombstone (alive=false) to avoid O(n) VecDeque remove + index rebuild.
/// Ref: Zhang et al., "SIEVE is Simpler than LRU", NSDI 2024.
pub struct SievePolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    /// Circular queue with tombstones.
    queue: VecDeque<SieveEntry>,
    /// Fast lookup: cache_key → index in queue.
    pub(crate) index: HashMap<String, usize>,
    /// Hand position for eviction scan.
    hand: usize,
    /// Count of tombstoned (dead) entries for periodic compaction.
    tombstones: usize,
}

impl SievePolicy {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            capacity_bytes,
            used_bytes: 0,
            ttl_seconds: 3600,
            queue: VecDeque::new(),
            index: HashMap::new(),
            hand: 0,
            tombstones: 0,
        }
    }

    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    fn evict_one(&mut self) -> bool {
        let len = self.queue.len();
        if len == 0 {
            return false;
        }
        let live_count = len - self.tombstones;
        if live_count == 0 {
            return false;
        }
        let mut scanned = 0;
        while scanned < len {
            let pos = self.hand % len;
            self.hand = (self.hand + 1) % len;
            let entry = &mut self.queue[pos];
            if !entry.alive {
                scanned += 1;
                continue;
            }
            if entry.visited {
                entry.visited = false;
                scanned += 1;
                continue;
            }
            // Evict: tombstone this entry
            let size = entry.size;
            entry.alive = false;
            self.index.remove(&entry.key);
            self.used_bytes -= size;
            self.tombstones += 1;
            return true;
        }
        // All live entries were visited — evict current hand entry
        let pos = self.hand % len;
        // Find next live entry from hand
        for i in 0..len {
            let p = (pos + i) % len;
            let entry = &mut self.queue[p];
            if entry.alive {
                let size = entry.size;
                entry.alive = false;
                self.index.remove(&entry.key);
                self.used_bytes -= size;
                self.tombstones += 1;
                self.hand = (p + 1) % len;
                return true;
            }
        }
        false
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes {
            if !self.evict_one() {
                break;
            }
        }
        self.maybe_compact();
    }

    /// Compact tombstones when they exceed half the queue.
    fn maybe_compact(&mut self) {
        if self.tombstones > 0 && self.tombstones * 2 > self.queue.len() {
            let mut new_queue = VecDeque::with_capacity(self.queue.len() - self.tombstones);
            self.index.clear();
            for entry in self.queue.drain(..) {
                if entry.alive {
                    let idx = new_queue.len();
                    self.index.insert(entry.key.clone(), idx);
                    new_queue.push_back(entry);
                }
            }
            self.queue = new_queue;
            self.tombstones = 0;
            self.hand = self.hand.min(self.queue.len().saturating_sub(1));
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
            let entry = &mut self.queue[idx];
            // Check stale before marking visited
            let stale = check_stale(
                entry.insert_time,
                entry.version.as_deref(),
                event,
                self.ttl_seconds,
            );
            entry.visited = true;
            if stale {
                entry.insert_time = event.timestamp;
                entry.version = event.version_or_etag.clone();
                CacheOutcome::StaleHit
            } else {
                CacheOutcome::Hit
            }
        } else {
            // Miss: insert at tail
            let size = event.object_size_bytes;
            if size > self.capacity_bytes {
                return CacheOutcome::Miss;
            }
            self.evict_until_fits(size);
            let idx = self.queue.len();
            self.queue.push_back(SieveEntry {
                key: event.cache_key.clone(),
                size,
                visited: false,
                alive: true,
                insert_time: event.timestamp,
                version: event.version_or_etag.clone(),
            });
            self.index.insert(event.cache_key.clone(), idx);
            self.used_bytes += size;
            CacheOutcome::Miss
        }
    }
}

// ── S3-FIFO (SOSP 2023) ────────────────────────────────────────────

/// Entry metadata for S3-FIFO queues.
pub(crate) struct S3FifoEntry {
    size: u64,
    freq: u8,
    insert_time: DateTime<Utc>,
    version: Option<String>,
}

/// S3-FIFO: three static FIFO queues with quick demotion.
///
/// Small (10%): probationary. Objects enter here.
/// Main (90%): promoted from Small on hit.
/// Ghost: metadata-only for recently evicted from Small.
///
/// Data (size, freq) lives in HashMaps for O(1) lookup/update.
/// VecDeque<String> tracks FIFO order only.
/// Ref: Yang et al., "FIFO Queues are All You Need", SOSP 2023.
pub struct S3FifoPolicy {
    small_capacity: u64,
    main_capacity: u64,
    small_used: u64,
    main_used: u64,
    ttl_seconds: u64,
    /// Small FIFO order
    small_order: VecDeque<String>,
    /// Main FIFO order
    main_order: VecDeque<String>,
    /// Small entry data: cache_key → (size, freq)
    pub(crate) in_small: HashMap<String, S3FifoEntry>,
    /// Main entry data: cache_key → (size, freq)
    pub(crate) in_main: HashMap<String, S3FifoEntry>,
    /// Ghost set: recently evicted from Small (metadata only)
    ghost: std::collections::HashSet<String>,
    ghost_max: usize,
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
            ttl_seconds: 3600,
            small_order: VecDeque::new(),
            main_order: VecDeque::new(),
            in_small: HashMap::new(),
            in_main: HashMap::new(),
            ghost: std::collections::HashSet::new(),
            ghost_max: 1000,
        }
    }

    pub fn with_ttl(mut self, ttl_seconds: u64) -> Self {
        self.ttl_seconds = ttl_seconds;
        self
    }

    fn evict_small(&mut self) {
        while let Some(key) = self.small_order.pop_front() {
            if let Some(entry) = self.in_small.remove(&key) {
                self.small_used -= entry.size;
                if entry.freq > 0 {
                    // Promote to main (carry insert_time/version)
                    self.insert_main_with_meta(&key, entry.size, entry.insert_time, entry.version);
                } else {
                    // Quick demotion: discard (one-hit wonder)
                    self.ghost.insert(key);
                    if self.ghost.len() > self.ghost_max {
                        if let Some(old) = self.ghost.iter().next().cloned() {
                            self.ghost.remove(&old);
                        }
                    }
                }
                return;
            }
            // Stale order entry (already evicted/promoted), skip
        }
    }

    /// Evict one entry from main queue. Returns true if bytes were freed.
    fn evict_main(&mut self) -> bool {
        while let Some(key) = self.main_order.pop_front() {
            if let Some(entry) = self.in_main.remove(&key) {
                self.main_used -= entry.size;
                if entry.freq > 0 {
                    // Re-insert with decremented freq (second chance)
                    self.main_order.push_back(key.clone());
                    self.in_main.insert(
                        key,
                        S3FifoEntry {
                            size: entry.size,
                            freq: entry.freq - 1,
                            insert_time: entry.insert_time,
                            version: entry.version,
                        },
                    );
                    self.main_used += entry.size;
                } else {
                    return true; // Evicted
                }
            }
        }
        false
    }

    fn insert_main_with_meta(
        &mut self,
        key: &str,
        size: u64,
        insert_time: DateTime<Utc>,
        version: Option<String>,
    ) {
        while self.main_used + size > self.main_capacity && !self.in_main.is_empty() {
            if !self.evict_main() {
                break;
            }
        }
        self.main_order.push_back(key.to_string());
        self.in_main.insert(
            key.to_string(),
            S3FifoEntry {
                size,
                freq: 0,
                insert_time,
                version,
            },
        );
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

        // Check if in small queue — O(1) lookup + freq update + stale check
        if let Some(entry) = self.in_small.get_mut(&event.cache_key) {
            let stale = check_stale(
                entry.insert_time,
                entry.version.as_deref(),
                event,
                self.ttl_seconds,
            );
            entry.freq = entry.freq.saturating_add(1).min(3);
            if stale {
                entry.insert_time = event.timestamp;
                entry.version = event.version_or_etag.clone();
                return CacheOutcome::StaleHit;
            }
            return CacheOutcome::Hit;
        }

        // Check if in main queue — O(1) lookup + freq update + stale check
        if let Some(entry) = self.in_main.get_mut(&event.cache_key) {
            let stale = check_stale(
                entry.insert_time,
                entry.version.as_deref(),
                event,
                self.ttl_seconds,
            );
            entry.freq = entry.freq.saturating_add(1).min(3);
            if stale {
                entry.insert_time = event.timestamp;
                entry.version = event.version_or_etag.clone();
                return CacheOutcome::StaleHit;
            }
            return CacheOutcome::Hit;
        }

        // Cache miss — insert to small
        let size = event.object_size_bytes;
        if size > self.small_capacity + self.main_capacity {
            return CacheOutcome::Miss;
        }

        while self.small_used + size > self.small_capacity && !self.in_small.is_empty() {
            self.evict_small();
        }

        let initial_freq = if self.ghost.contains(&event.cache_key) {
            self.ghost.remove(&event.cache_key);
            1 // Ghost hit → start with freq=1 (will be promoted on next eviction)
        } else {
            0
        };

        self.small_order.push_back(event.cache_key.clone());
        self.in_small.insert(
            event.cache_key.clone(),
            S3FifoEntry {
                size,
                freq: initial_freq,
                insert_time: event.timestamp,
                version: event.version_or_etag.clone(),
            },
        );
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
