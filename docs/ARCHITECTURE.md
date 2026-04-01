# quant-cache Architecture

## Crate Dependency Graph

```
qc-model (data types, no dependencies on other crates)
    ↑
qc-solver (depends on qc-model)
    ↑
qc-simulate (depends on qc-model; dev-depends on qc-solver for acceptance tests)
    ↑
qc-cli (depends on all three)
```

## Data Flow

```
[Input: CSV trace or synthetic generator]
            │
            ▼
    RequestTraceEvent[]
            │
            ▼
   aggregate_features()          ← qc-simulate/synthetic.rs
            │
            ▼
    ObjectFeatures[]
            │
            ▼
   BenefitCalculator::score()    ← qc-solver/score.rs
            │
            ▼
    ScoredObject[]
            │
    ┌───────┴───────┐
    ▼               ▼
GreedySolver    ExactIlpSolver   ← qc-solver/{greedy,ilp}.rs
    │               │
    ▼               ▼
SolverResult (PolicyDecision[], objective_value, solve_time, shadow_price)
    │
    ▼
PolicyFile (JSON with SolverMetadata + decisions)
    │
    ▼
TraceReplayEngine::replay()      ← qc-simulate/engine.rs
    │
    ▼
MetricsSummary (hit_ratio, byte_hit_ratio, cost_savings, objective_value, stale_rate, ...)
```

## Module Responsibilities

### qc-model

Pure data types. No business logic beyond `StalePenaltyClass::to_cost()`.

| File | Types |
|------|-------|
| trace.rs | `RequestTraceEvent`, `CacheStatus` |
| object.rs | `ObjectFeatures`, `ScoredObject`, `ScoreBreakdown` |
| policy.rs | `PolicyDecision`, `PolicyFile`, `SolverMetadata` |
| scenario.rs | `ScenarioConfig`, `FreshnessModel`, `StalePenaltyConfig`, `StaleCostOverrides`, `CapacityConstraint` |
| metrics.rs | `MetricsSummary` |
| preset.rs | `Preset` enum (ecommerce, media, api) |
| error.rs | `ModelError` |

### qc-solver

Scoring and optimization. Separated into:

- **score.rs** — `BenefitCalculator`: transforms `ObjectFeatures + ScenarioConfig → ScoredObject`
- **solver.rs** — `Solver` trait: `solve(&[ScoredObject], &CapacityConstraint) → SolverResult`
- **greedy.rs** — `GreedySolver`: ratio + pure-benefit dual strategy, O(n log n)
- **ilp.rs** — `ExactIlpSolver`: HiGHS via good_lp, exact for n < 10,000

Key design: **Scoring and solving are separated.** Solver receives pre-scored objects
and only knows about `net_benefit` and `size_bytes`. This allows:
- Different scoring functions without changing solvers
- Solver-agnostic testing with manually scored objects
- Clean V2 extension path (add `QuadraticSolver` without touching scoring)

### qc-simulate

Trace replay and evaluation. Components:

- **engine.rs** — `TraceReplayEngine`: replays events against any `CachePolicy`, collects `MetricsSummary`
- **baselines.rs** — `LruPolicy`, `GdsfPolicy`, `StaticPolicy` implementing `CachePolicy` trait
- **comparator.rs** — `Comparator`: runs multiple policies on same trace, produces `ComparisonReport`
- **synthetic.rs** — `generate()`: synthetic trace with Zipf popularity, heterogeneous costs; `aggregate_features()`: trace → ObjectFeatures

All policies implement stale detection via TTL expiry and version mismatch.

`ReplayEconConfig` provides per-object stale penalty lookup matching the solver's
per-object `stale_penalty_class`, ensuring replay objective tracks solver objective.

### qc-cli

CLI subcommands (`clap` derive):

| Command | Function |
|---------|----------|
| `qc generate` | Synthetic trace → CSV |
| `qc optimize` | CSV → PolicyFile JSON (scoring + solving) |
| `qc simulate` | CSV + PolicyFile → MetricsSummary |
| `qc compare` | CSV → comparison table (LRU, GDSF, EconomicGreedy, optional ILP) |

Supports TOML config files and preset profiles.

## Key Design Decisions

### Why Scoring and Solving Are Separated

The `Solver` trait receives `ScoredObject` (containing `net_benefit` and `size_bytes`)
not `ObjectFeatures`. This means:
1. Solver never sees `latency_value_per_ms` or `stale_penalty_class`
2. Different scoring models (V1 linear, V1.5 reuse-distance) don't affect solver code
3. Property-based tests can generate arbitrary scored objects without valid features

### Why Static Policy vs Online Baselines

EconomicGreedy produces a **fixed set** of cache keys (offline optimization).
LRU and GDSF are **online** policies that adapt during replay.
This is a fundamental difference, not a bug:
- EconomicGreedy is optimal within its model (verified by ILP)
- LRU/GDSF may achieve higher hit rates by adapting to temporal patterns
- The comparison validates the economic formulation, not runtime supremacy

### Why Per-Object Stale Penalty

Different content types have different freshness requirements.
A stale price on a product page ($0.10/event) is more costly than a stale CSS file ($0.001/event).
The scoring function uses `stale_penalty_class.to_cost_with_overrides()` per object,
and the replay engine mirrors this via `ReplayEconConfig::from_features_with_overrides()`.
