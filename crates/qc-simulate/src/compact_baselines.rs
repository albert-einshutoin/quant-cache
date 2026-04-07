use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use qc_model::compact_trace::CompactTraceEvent;
use qc_model::intern::NONE_ID;

use crate::engine::{CacheOutcome, CompactCachePolicy};

// ── Stale detection helper (compact) ──────────────────────────────────

/// Check if a cached item is stale by TTL expiry or version_or_etag_id mismatch.
///
/// `version_or_etag_id == NONE_ID` means no version is known; stale check on
/// version is skipped in that case (mirrors the String-based `check_stale`
/// which only compares when both sides are `Some`).
fn check_stale_compact(
    insert_time: DateTime<Utc>,
    cached_version_id: u32,
    event: &CompactTraceEvent,
    ttl_seconds: u64,
) -> bool {
    let age = (event.timestamp - insert_time).num_seconds();
    if age > ttl_seconds as i64 {
        return true;
    }
    // Only compare versions when both sides carry a real ID (non-NONE).
    if cached_version_id != NONE_ID
        && event.version_or_etag_id != NONE_ID
        && cached_version_id != event.version_or_etag_id
    {
        return true;
    }
    false
}

// ── CompactLruPolicy ─────────────────────────────────────────────────

struct LruEntry {
    size: u64,
    generation: u64,
    insert_time: DateTime<Utc>,
    version_id: u32,
}

/// Compact LRU policy using u32 cache_key_id instead of String keys.
///
/// Uses a generation counter + BTreeMap for O(log n) eviction, mirroring
/// the String-based `LruPolicy` in `baselines.rs`.
pub struct CompactLruPolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    generation: u64,
    entries: HashMap<u32, LruEntry>,
    /// generation → cache_key_id for O(log n) min-generation eviction.
    order: BTreeMap<u64, u32>,
}

impl CompactLruPolicy {
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

    /// Current bytes used in cache. Useful for testing capacity invariants.
    pub fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    fn touch(&mut self, key_id: u32) {
        if let Some(entry) = self.entries.get_mut(&key_id) {
            self.order.remove(&entry.generation);
            self.generation += 1;
            entry.generation = self.generation;
            self.order.insert(self.generation, key_id);
        }
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes {
            if let Some((&gen, _)) = self.order.iter().next() {
                let key_id = self.order.remove(&gen).unwrap();
                if let Some(entry) = self.entries.remove(&key_id) {
                    self.used_bytes -= entry.size;
                }
            } else {
                break;
            }
        }
    }

    fn insert(&mut self, event: &CompactTraceEvent) {
        let size = event.object_size_bytes;
        if size > self.capacity_bytes {
            return;
        }
        self.evict_until_fits(size);
        self.generation += 1;
        self.order.insert(self.generation, event.cache_key_id);
        self.entries.insert(
            event.cache_key_id,
            LruEntry {
                size,
                generation: self.generation,
                insert_time: event.timestamp,
                version_id: event.version_or_etag_id,
            },
        );
        self.used_bytes += size;
    }
}

impl CompactCachePolicy for CompactLruPolicy {
    fn name(&self) -> &str {
        "CompactLRU"
    }

    fn on_request(&mut self, event: &CompactTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        if let Some(entry) = self.entries.get(&event.cache_key_id) {
            let stale =
                check_stale_compact(entry.insert_time, entry.version_id, event, self.ttl_seconds);
            self.touch(event.cache_key_id);

            if stale {
                if let Some(entry) = self.entries.get_mut(&event.cache_key_id) {
                    entry.insert_time = event.timestamp;
                    entry.version_id = event.version_or_etag_id;
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

// ── CompactSievePolicy ───────────────────────────────────────────────

/// SIEVE entry stored in the circular queue (compact variant).
struct CompactSieveEntry {
    key_id: u32,
    size: u64,
    visited: bool,
    alive: bool,
    insert_time: DateTime<Utc>,
    version_id: u32,
}

/// Compact SIEVE eviction policy using u32 cache_key_id.
///
/// Tombstone-based VecDeque with u32 keys, mirroring `SievePolicy`.
/// Ref: Zhang et al., "SIEVE is Simpler than LRU", NSDI 2024.
pub struct CompactSievePolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    /// Circular queue with tombstones.
    queue: VecDeque<CompactSieveEntry>,
    /// Fast lookup: cache_key_id → index in queue.
    index: HashMap<u32, usize>,
    /// Hand position for eviction scan.
    hand: usize,
    /// Count of tombstoned (dead) entries for periodic compaction.
    tombstones: usize,
}

impl CompactSievePolicy {
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
            self.index.remove(&entry.key_id);
            self.used_bytes -= size;
            self.tombstones += 1;
            return true;
        }
        // All live entries were visited — evict next live entry from hand
        let pos = self.hand % len;
        for i in 0..len {
            let p = (pos + i) % len;
            let entry = &mut self.queue[p];
            if entry.alive {
                let size = entry.size;
                entry.alive = false;
                self.index.remove(&entry.key_id);
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
                    self.index.insert(entry.key_id, idx);
                    new_queue.push_back(entry);
                }
            }
            self.queue = new_queue;
            self.tombstones = 0;
            self.hand = self.hand.min(self.queue.len().saturating_sub(1));
        }
    }
}

impl CompactCachePolicy for CompactSievePolicy {
    fn name(&self) -> &str {
        "CompactSIEVE"
    }

    fn on_request(&mut self, event: &CompactTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }

        if let Some(&idx) = self.index.get(&event.cache_key_id) {
            let entry = &mut self.queue[idx];
            let stale =
                check_stale_compact(entry.insert_time, entry.version_id, event, self.ttl_seconds);
            entry.visited = true;
            if stale {
                entry.insert_time = event.timestamp;
                entry.version_id = event.version_or_etag_id;
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
            self.queue.push_back(CompactSieveEntry {
                key_id: event.cache_key_id,
                size,
                visited: false,
                alive: true,
                insert_time: event.timestamp,
                version_id: event.version_or_etag_id,
            });
            self.index.insert(event.cache_key_id, idx);
            self.used_bytes += size;
            CacheOutcome::Miss
        }
    }
}

// ── CompactStaticPolicy ──────────────────────────────────────────────

/// Compact static cache policy: a fixed HashSet<u32> of cache_key_ids.
///
/// Tracks insertion time and version_or_etag_id for stale detection,
/// mirroring `StaticPolicy` in `baselines.rs`.
pub struct CompactStaticPolicy {
    cached_keys: HashSet<u32>,
    name: String,
    ttl_seconds: u64,
    insert_times: HashMap<u32, DateTime<Utc>>,
    insert_versions: HashMap<u32, u32>,
}

impl CompactStaticPolicy {
    pub fn new(cached_keys: impl IntoIterator<Item = u32>) -> Self {
        Self {
            cached_keys: cached_keys.into_iter().collect(),
            name: "CompactEconomicGreedy".to_string(),
            ttl_seconds: 3600,
            insert_times: HashMap::new(),
            insert_versions: HashMap::new(),
        }
    }

    pub fn new_with_name(cached_keys: impl IntoIterator<Item = u32>, name: &str) -> Self {
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

impl CompactCachePolicy for CompactStaticPolicy {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_request(&mut self, event: &CompactTraceEvent) -> CacheOutcome {
        if !event.eligible_for_cache {
            return CacheOutcome::Bypass;
        }
        if !self.cached_keys.contains(&event.cache_key_id) {
            return CacheOutcome::Miss;
        }

        let stale = if let Some(&insert_time) = self.insert_times.get(&event.cache_key_id) {
            let cached_ver = self
                .insert_versions
                .get(&event.cache_key_id)
                .copied()
                .unwrap_or(NONE_ID);
            check_stale_compact(insert_time, cached_ver, event, self.ttl_seconds)
        } else {
            false // first access, not stale
        };

        // Refresh on access
        self.insert_times
            .insert(event.cache_key_id, event.timestamp);
        self.insert_versions
            .insert(event.cache_key_id, event.version_or_etag_id);

        if stale {
            CacheOutcome::StaleHit
        } else {
            CacheOutcome::Hit
        }
    }
}

// ── CompactBeladyPolicy ──────────────────────────────────────────────

struct CompactBeladyEntry {
    size: u64,
    insert_time: DateTime<Utc>,
    version_id: u32,
}

/// Compact Belady (MIN) oracle policy using u32 cache_key_id.
///
/// Requires the full trace upfront to build a future-access index.
/// On eviction, removes the object whose next access is farthest in the future.
pub struct CompactBeladyPolicy {
    capacity_bytes: u64,
    used_bytes: u64,
    ttl_seconds: u64,
    /// cache_key_id → entry metadata
    entries: HashMap<u32, CompactBeladyEntry>,
    /// cache_key_id → queue of future access positions (indices into the trace)
    future_accesses: HashMap<u32, VecDeque<usize>>,
    /// Current position in the trace
    current_pos: usize,
}

impl CompactBeladyPolicy {
    /// Build a Belady policy from the full compact trace.
    /// Must be called before replay.
    pub fn new(events: &[CompactTraceEvent], capacity_bytes: u64) -> Self {
        let mut future_accesses: HashMap<u32, VecDeque<usize>> = HashMap::new();
        for (i, event) in events.iter().enumerate() {
            if event.eligible_for_cache {
                future_accesses
                    .entry(event.cache_key_id)
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

    /// Next access position for a cache_key_id after the current position.
    /// Drains consumed positions from the front for O(1) amortized lookup.
    fn next_access(&mut self, key_id: u32) -> usize {
        if let Some(queue) = self.future_accesses.get_mut(&key_id) {
            while queue.front().is_some_and(|&pos| pos <= self.current_pos) {
                queue.pop_front();
            }
            if let Some(&pos) = queue.front() {
                return pos;
            }
        }
        usize::MAX // never accessed again
    }

    fn evict_until_fits(&mut self, needed: u64) {
        while self.used_bytes + needed > self.capacity_bytes {
            // Collect key_ids first to avoid borrow conflict with next_access
            let key_ids: Vec<u32> = self.entries.keys().copied().collect();
            let victim = key_ids
                .iter()
                .map(|&k| (k, self.next_access(k)))
                .max_by_key(|(_, next)| *next);

            if let Some((key_id, _)) = victim {
                if let Some(entry) = self.entries.remove(&key_id) {
                    self.used_bytes -= entry.size;
                }
            } else {
                break;
            }
        }
    }
}

impl CompactCachePolicy for CompactBeladyPolicy {
    fn name(&self) -> &str {
        "CompactBelady"
    }

    fn on_request(&mut self, event: &CompactTraceEvent) -> CacheOutcome {
        // Consume the current position from future_accesses
        if let Some(queue) = self.future_accesses.get_mut(&event.cache_key_id) {
            while queue.front().is_some_and(|&pos| pos <= self.current_pos) {
                queue.pop_front();
            }
        }

        if !event.eligible_for_cache {
            self.current_pos += 1;
            return CacheOutcome::Bypass;
        }

        let result = if self.entries.contains_key(&event.cache_key_id) {
            let stale = {
                let entry = self.entries.get(&event.cache_key_id).unwrap();
                check_stale_compact(entry.insert_time, entry.version_id, event, self.ttl_seconds)
            };
            if stale {
                if let Some(entry) = self.entries.get_mut(&event.cache_key_id) {
                    entry.insert_time = event.timestamp;
                    entry.version_id = event.version_or_etag_id;
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
                    event.cache_key_id,
                    CompactBeladyEntry {
                        size,
                        insert_time: event.timestamp,
                        version_id: event.version_or_etag_id,
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

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use qc_model::intern::NONE_ID;

    fn make_event(cache_key_id: u32, size: u64, ts_secs: i64, eligible: bool) -> CompactTraceEvent {
        CompactTraceEvent {
            timestamp: Utc.timestamp_opt(ts_secs, 0).unwrap(),
            cache_key_id,
            object_id_id: cache_key_id,
            object_size_bytes: size,
            response_bytes: 0,
            has_response_bytes: false,
            origin_fetch_cost: 0.01,
            response_latency_ms: 10.0,
            status_code: 200,
            content_type_id: NONE_ID,
            version_or_etag_id: NONE_ID,
            region_id: NONE_ID,
            eligible_for_cache: eligible,
        }
    }

    fn make_event_with_version(
        cache_key_id: u32,
        size: u64,
        ts_secs: i64,
        version_id: u32,
    ) -> CompactTraceEvent {
        CompactTraceEvent {
            version_or_etag_id: version_id,
            ..make_event(cache_key_id, size, ts_secs, true)
        }
    }

    // ── LRU tests ────────────────────────────────────────────────────

    #[test]
    fn lru_miss_then_hit() {
        let mut policy = CompactLruPolicy::new(1024);
        let e1 = make_event(1, 512, 1000, true);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Miss);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Hit);
    }

    #[test]
    fn lru_bypass_ineligible() {
        let mut policy = CompactLruPolicy::new(1024);
        let e = make_event(1, 512, 1000, false);
        assert_eq!(policy.on_request(&e), CacheOutcome::Bypass);
    }

    #[test]
    fn lru_evicts_lru_entry() {
        let mut policy = CompactLruPolicy::new(1000);
        let e1 = make_event(1, 500, 1000, true);
        let e2 = make_event(2, 500, 1001, true);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Miss);
        assert_eq!(policy.on_request(&e2), CacheOutcome::Miss);

        // Touch key 1 to make it most recent
        assert_eq!(policy.on_request(&e1), CacheOutcome::Hit);

        // Insert key 3 — key 2 (LRU) should be evicted
        let e3 = make_event(3, 500, 1002, true);
        assert_eq!(policy.on_request(&e3), CacheOutcome::Miss);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Hit); // still in cache
        assert_eq!(policy.on_request(&e2), CacheOutcome::Miss); // evicted
    }

    #[test]
    fn lru_ttl_stale_detection() {
        let mut policy = CompactLruPolicy::new(1024).with_ttl(100);
        let e1 = make_event(1, 256, 1000, true);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Miss);

        let e_stale = make_event(1, 256, 1200, true); // 200s later > 100s TTL
        assert_eq!(policy.on_request(&e_stale), CacheOutcome::StaleHit);

        let e_fresh = make_event(1, 256, 1250, true); // 50s after stale refresh
        assert_eq!(policy.on_request(&e_fresh), CacheOutcome::Hit);
    }

    #[test]
    fn lru_version_stale_detection() {
        let mut policy = CompactLruPolicy::new(1024).with_ttl(9999);
        let e_v1 = make_event_with_version(1, 256, 1000, 10);
        assert_eq!(policy.on_request(&e_v1), CacheOutcome::Miss);

        let e_v2 = make_event_with_version(1, 256, 1001, 11); // different version
        assert_eq!(policy.on_request(&e_v2), CacheOutcome::StaleHit);

        let e_v2_again = make_event_with_version(1, 256, 1002, 11); // same version now
        assert_eq!(policy.on_request(&e_v2_again), CacheOutcome::Hit);
    }

    #[test]
    fn lru_oversized_object_not_inserted() {
        let mut policy = CompactLruPolicy::new(100);
        let e = make_event(1, 200, 1000, true); // 200 > 100 capacity
        assert_eq!(policy.on_request(&e), CacheOutcome::Miss);
        assert_eq!(policy.used_bytes(), 0);
    }

    // ── SIEVE tests ───────────────────────────────────────────────────

    #[test]
    fn sieve_miss_then_hit() {
        let mut policy = CompactSievePolicy::new(1024);
        let e = make_event(1, 256, 1000, true);
        assert_eq!(policy.on_request(&e), CacheOutcome::Miss);
        assert_eq!(policy.on_request(&e), CacheOutcome::Hit);
    }

    #[test]
    fn sieve_bypass_ineligible() {
        let mut policy = CompactSievePolicy::new(1024);
        let e = make_event(1, 256, 1000, false);
        assert_eq!(policy.on_request(&e), CacheOutcome::Bypass);
    }

    #[test]
    fn sieve_evicts_unvisited() {
        // capacity = 1000, insert two 500-byte objects
        let mut policy = CompactSievePolicy::new(1000);
        let e1 = make_event(1, 500, 1000, true);
        let e2 = make_event(2, 500, 1001, true);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Miss); // key 1 in, unvisited
        assert_eq!(policy.on_request(&e2), CacheOutcome::Miss); // key 2 in, unvisited

        // key 1 gets visited bit set
        assert_eq!(policy.on_request(&e1), CacheOutcome::Hit);

        // insert key 3: needs to evict. hand scans: key 1 visited → clear; key 2 unvisited → evict
        let e3 = make_event(3, 500, 1002, true);
        assert_eq!(policy.on_request(&e3), CacheOutcome::Miss);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Hit); // still alive
        assert_eq!(policy.on_request(&e2), CacheOutcome::Miss); // evicted
    }

    #[test]
    fn sieve_ttl_stale_detection() {
        let mut policy = CompactSievePolicy::new(1024).with_ttl(100);
        let e1 = make_event(1, 256, 1000, true);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Miss);

        let e_stale = make_event(1, 256, 1200, true);
        assert_eq!(policy.on_request(&e_stale), CacheOutcome::StaleHit);

        let e_fresh = make_event(1, 256, 1250, true);
        assert_eq!(policy.on_request(&e_fresh), CacheOutcome::Hit);
    }

    #[test]
    fn sieve_oversized_object_not_inserted() {
        let mut policy = CompactSievePolicy::new(100);
        let e = make_event(1, 200, 1000, true);
        assert_eq!(policy.on_request(&e), CacheOutcome::Miss);
        assert_eq!(policy.used_bytes, 0);
    }

    // ── Static tests ──────────────────────────────────────────────────

    #[test]
    fn static_policy_hit_and_miss() {
        let mut policy = CompactStaticPolicy::new([1u32, 2u32]);
        let e1 = make_event(1, 256, 1000, true);
        let e3 = make_event(3, 256, 1000, true);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Hit);
        assert_eq!(policy.on_request(&e3), CacheOutcome::Miss);
    }

    #[test]
    fn static_policy_bypass_ineligible() {
        let mut policy = CompactStaticPolicy::new([1u32]);
        let e = make_event(1, 256, 1000, false);
        assert_eq!(policy.on_request(&e), CacheOutcome::Bypass);
    }

    #[test]
    fn static_policy_ttl_stale() {
        let mut policy = CompactStaticPolicy::new([1u32]).with_ttl(100);
        let e1 = make_event(1, 256, 1000, true);
        assert_eq!(policy.on_request(&e1), CacheOutcome::Hit); // first access: no insert_time yet
        let e_stale = make_event(1, 256, 1200, true);
        assert_eq!(policy.on_request(&e_stale), CacheOutcome::StaleHit);
        let e_fresh = make_event(1, 256, 1250, true);
        assert_eq!(policy.on_request(&e_fresh), CacheOutcome::Hit);
    }

    #[test]
    fn static_policy_custom_name() {
        let policy = CompactStaticPolicy::new_with_name([1u32], "MyPolicy");
        assert_eq!(policy.name(), "MyPolicy");
    }

    // ── Belady tests ──────────────────────────────────────────────────

    #[test]
    fn belady_miss_then_hit() {
        let events = vec![
            make_event(1, 256, 1000, true),
            make_event(1, 256, 1001, true),
        ];
        let mut policy = CompactBeladyPolicy::new(&events, 1024);
        assert_eq!(policy.on_request(&events[0]), CacheOutcome::Miss);
        assert_eq!(policy.on_request(&events[1]), CacheOutcome::Hit);
    }

    #[test]
    fn belady_bypass_ineligible() {
        let events = vec![make_event(1, 256, 1000, false)];
        let mut policy = CompactBeladyPolicy::new(&events, 1024);
        assert_eq!(policy.on_request(&events[0]), CacheOutcome::Bypass);
    }

    #[test]
    fn belady_evicts_farthest_future_access() {
        // capacity = 1000
        // key 1 (500B): next access at pos 3
        // key 2 (500B): next access at pos 4 (farther) → should be evicted
        // key 3 (500B): inserted at pos 2, forces eviction
        let events = vec![
            make_event(1, 500, 1000, true), // pos 0
            make_event(2, 500, 1001, true), // pos 1
            make_event(3, 500, 1002, true), // pos 2: insert, evict key2 (farther)
            make_event(1, 500, 1003, true), // pos 3: key1 hit
            make_event(2, 500, 1004, true), // pos 4: key2 miss (was evicted)
        ];
        let mut policy = CompactBeladyPolicy::new(&events, 1000);
        assert_eq!(policy.on_request(&events[0]), CacheOutcome::Miss); // key1 in
        assert_eq!(policy.on_request(&events[1]), CacheOutcome::Miss); // key2 in
        assert_eq!(policy.on_request(&events[2]), CacheOutcome::Miss); // key3 in, key2 evicted
        assert_eq!(policy.on_request(&events[3]), CacheOutcome::Hit); // key1 hit
        assert_eq!(policy.on_request(&events[4]), CacheOutcome::Miss); // key2 evicted
    }

    #[test]
    fn belady_ttl_stale_detection() {
        let events = vec![
            make_event(1, 256, 1000, true),
            make_event(1, 256, 1200, true), // 200s later > 100s TTL
            make_event(1, 256, 1250, true), // 50s after stale refresh
        ];
        let mut policy = CompactBeladyPolicy::new(&events, 1024).with_ttl(100);
        assert_eq!(policy.on_request(&events[0]), CacheOutcome::Miss);
        assert_eq!(policy.on_request(&events[1]), CacheOutcome::StaleHit);
        assert_eq!(policy.on_request(&events[2]), CacheOutcome::Hit);
    }

    #[test]
    fn belady_oversized_object_stays_miss() {
        let events = vec![make_event(1, 2000, 1000, true)];
        let mut policy = CompactBeladyPolicy::new(&events, 1000);
        assert_eq!(policy.on_request(&events[0]), CacheOutcome::Miss);
        assert_eq!(policy.used_bytes, 0);
    }

    // ── check_stale_compact unit tests ───────────────────────────────

    #[test]
    fn check_stale_compact_ttl_expired() {
        let insert = Utc.timestamp_opt(1000, 0).unwrap();
        let event = make_event(1, 256, 1200, true); // 200s > 100s TTL
        assert!(check_stale_compact(insert, NONE_ID, &event, 100));
    }

    #[test]
    fn check_stale_compact_ttl_not_expired() {
        let insert = Utc.timestamp_opt(1000, 0).unwrap();
        let event = make_event(1, 256, 1050, true); // 50s < 100s TTL
        assert!(!check_stale_compact(insert, NONE_ID, &event, 100));
    }

    #[test]
    fn check_stale_compact_version_mismatch() {
        let insert = Utc.timestamp_opt(1000, 0).unwrap();
        let event = make_event_with_version(1, 256, 1001, 20);
        assert!(check_stale_compact(insert, 10, &event, 9999)); // version 10 vs 20
    }

    #[test]
    fn check_stale_compact_version_match() {
        let insert = Utc.timestamp_opt(1000, 0).unwrap();
        let event = make_event_with_version(1, 256, 1001, 10);
        assert!(!check_stale_compact(insert, 10, &event, 9999)); // same version
    }

    #[test]
    fn check_stale_compact_none_version_skipped() {
        // When either side is NONE_ID, version check is skipped
        let insert = Utc.timestamp_opt(1000, 0).unwrap();
        let event = make_event(1, 256, 1001, true); // version_or_etag_id = NONE_ID
        assert!(!check_stale_compact(insert, NONE_ID, &event, 9999));
        // cached_version = NONE_ID, event version = non-NONE: still no stale
        let event_with_ver = make_event_with_version(1, 256, 1001, 5);
        assert!(!check_stale_compact(insert, NONE_ID, &event_with_ver, 9999));
    }
}
