# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-04-07

Phase E complete: parametric validation, quantum-inspired policy search, multi-vendor deployment.

### Added
- **Parametric validation suite** — 2,880+ parameter sweep across Zipf/capacity/objects/update_rate/scoring with 6 invariants (Belady ceiling, capacity, NaN, determinism, monotonicity, finite metrics)
- **QUBO formulation for PolicyIR DSL search** — binary variable encoding of policy dimensions, trace-driven interaction weight estimation, one-hot constraint enforcement
- **SA multi-restart policy search** — 3 diverse initial states for better exploration
- **Scorer trait** for V1/V2 scoring strategy dispatch (`V1Scorer`, `V2Scorer`, `create_scorer()`)
- **Unified `SolverResult`** type for all solvers (Greedy, ILP, SA/QUBO)
- Stale detection for SIEVE and S3-FIFO policies (TTL + version mismatch)
- `size_bytes` field on `PolicyDecision` for density-based admission
- `ScoreDensityThreshold` properly computes score/size in compile output
- Cache key normalization in Cloudflare Worker and Akamai EdgeWorker output
- `--time-window` properly propagated to `ScenarioConfig` for scoring
- CalibrationEngine tests (4) + NaN/Inf boundary tests (4)
- Fast comparison tests (Belady ceiling) + E2E pipeline test
- SA vs Grid quality comparison tests + QUBO DSL test
- CLI `--method qubo` for quantum-inspired policy search

### Changed
- **compile.rs split into per-CDN modules** — cloudflare.rs, cloudfront.rs, fastly.rs, akamai.rs (was 1,353 lines)
- **LRU** — O(1) promote via BTreeMap generation counter (was O(n) VecDeque scan)
- **GDSF** — O(log n) eviction via BTreeMap priority index (was O(n) HashMap scan)
- **SIEVE** — O(1) eviction via tombstone (was O(n) VecDeque remove + index rebuild)
- **S3-FIFO** — data/order separation fixes stale-index bug + O(1) freq update
- **Belady** — O(1) amortized `next_access` (was O(k) linear scan)
- Calibrate engine: proper train/val separation (train policy on train data, evaluate on val)
- `IrPolicy::key_cache` bounded at 100K entries to prevent unbounded growth
- Reuse distance: gap cap at 100K accesses (was unbounded O(n²))
- Co-access: pair limit at 50M (was unbounded)
- Group interactions: pre-select top sqrt(2k) members (was all-pairs O(n²))
- `TraceReplayEngine::replay` generic with `<P: CachePolicy + ?Sized>` for static dispatch
- CloudFront parser: bounded line reads via `take()` + `read_until` (OOM prevention)
- CSV reader: strict field count validation (removed `flexible(true)`)
- QUBO SA solver: `saturating_add/sub` for `used_bytes` (both initial greedy + SA loop)

### Fixed
- **Proptest `ilp_at_least_as_good_as_greedy`** — unique cache keys via `arb_problem()` + HiGHS MIP tolerance
- NaN propagation from `time_window=0` or malformed inputs clamped to 0.0
- `time_window_seconds * 1000` overflow guarded with `checked_mul`
- GDSF `compute_priority` division by zero on `size=0`
- GDSF priority NaN guard in BTreeMap encoding and inflation assignment
- S3-FIFO `evict_main` livelock when all entries have freq > 0
- SA search no longer mutates `capacity_bytes` (was a constraint, not a tunable)
- CloudFront parser clamps `time_taken` to 0..3600 range

## [0.1.0] - 2026-04-02

Initial release: Phases A-D complete.

### Added
- Economic knapsack evaluation framework (V1.0)
- GreedySolver + ExactIlpSolver
- LRU, GDSF, SIEVE, S3-FIFO, Belady baseline policies
- Synthetic trace generator (Zipf, heterogeneous costs, version changes)
- CloudFront log parser
- Policy IR type definitions + IrPolicy replay
- Policy search engine (grid + random)
- Deployment scaffold generator (Cloudflare, CloudFront, Fastly, Akamai)
- V2 reuse-distance-aware scoring
- QUBO SA solver with co-access/purge-group/origin-group interactions
- Calibration engine (coordinate descent)
- CLI with 11 subcommands
