# quant-cache Roadmap

**Version:** 1.0
**Date:** 2026-04-01
**Status:** Confirmed (Claude x Codex)

---

## Version Overview

```text
V1.0 ──→ V1.1 ──→ V1.5 ──→ V1.6 ──→ V2.0 ──→ V2.5 ──→ V3.0
 done    ingest   model     reuse    QUBO    provider  quantum
                  strength  distance          API
```

---

## V1.0 — Economic Knapsack (DONE)

- 0-1 knapsack formulation with economic objective ($/period)
- GreedySolver + ExactIlpSolver
- Trace replay engine with LRU/GDSF baselines
- Synthetic trace generator (heterogeneous costs, version changes)
- Per-object stale penalty with configurable overrides
- CLI: generate, optimize, simulate, compare
- Acceptance: gap median 0.01%, EconomicGreedy beats LRU 16/20

---

## V1.1 — Real Trace Validation

**Goal:** Validate V1 formulation against real CDN traffic

### Deliverables

| Item | Description |
|------|-------------|
| `qc import` CLI | Convert provider logs → canonical trace CSV |
| CloudFront parser | Map CloudFront fields → RequestTraceEvent |
| ProviderLogParser trait | Interface for future provider parsers |
| OriginCostConfig | Fallback chain: rule → content-type → latency → global default |
| Real trace benchmark | Run V1 pipeline on CloudFront logs, report results |

### CloudFront Field Mapping

| CloudFront | → RequestTraceEvent | Notes |
|------------|---------------------|-------|
| date + time | timestamp | |
| cs-uri-stem + cs-uri-query | cache_key | |
| cs-uri-stem | object_id | |
| sc-bytes | response_bytes | |
| x-edge-result-type | cache_status | Hit/RefreshHit→Hit, Miss→Miss, Error→Bypass |
| time-taken | response_latency_ms | |
| sc-status | status_code | 206→eligible_for_cache=false |
| (config) | origin_fetch_cost | From OriginCostConfig fallback chain |
| (aggregate) | object_size_bytes | max(response_bytes) per cache_key |

### Design Decisions

- RefreshHit → Hit (replay handles freshness)
- 206 Partial Content → eligible_for_cache=false by default
- update_rate from preset/external metadata (not inferred from logs)
- Cloudflare deferred to V1.2

---

## V1.5 — Model Strengthening

**Goal:** Improve model accuracy with Belady baseline, coefficient calibration,
and lazy-image integration groundwork

### Deliverables

| Item | Description |
|------|-------------|
| BeladyPolicy | Replay oracle — future-knowledge eviction for hit-rate ceiling |
| AutoCalibrationJob | Coordinate descent + random restarts for coefficient tuning |
| Train/validation split | Time-based split for overfitting prevention |
| Calibrate CLI | `qc calibrate` subcommand |
| lazy-image manifest | Schema design for variant manifest integration |

### Belady Implementation

- Pre-index trace: per cache_key future access position queue
- Online simulation with CachePolicy trait
- Standard Belady only (not EconomicBelady)
- Comparison axes:
  - EconomicGreedy vs ILP → optimization quality
  - EconomicGreedy vs Belady → static policy ceiling

### Calibration Design

- Method: coordinate descent + bounded random restarts
- Objective: maximize replay estimated_cost_savings
- Split: time-based (train: past 7 days, validation: last 1 day)
- Output: tuned config, validation score, sensitivity report

---

## V1.6 — Reuse Distance Scoring

**Goal:** Replace frequency-based demand estimation with reuse-distance-aware scoring

### Deliverables

| Item | Description |
|------|-------------|
| Reuse distance computation | Per cache_key reuse distance distribution from trace |
| BenefitCalculatorV2 | Scoring using reuse distance P50/P95 instead of raw request_count |
| ObjectFeatures extension | mean_reuse_distance, reuse_distance_p50, reuse_distance_p95 |
| A/B comparison | V1 scoring vs V2 scoring on same traces |

### Academic Basis

- Paper 15: PRP (Probabilistic Replacement Policy)
- Paper 16: Reuse Distance & Stream Detection (Keramidas, 2007)

---

## V2.0 — QUBO with Quadratic Terms

**Goal:** Introduce pairwise interactions for co-access, purge-group, origin-group

### Deliverables

| Item | Description |
|------|-------------|
| QuadraticProblem type | linear_terms + sparse PairwiseInteraction list |
| QuadraticSolver trait | Separate from linear Solver trait |
| Co-access extraction | Time-window co-occurrence counting from traces |
| SA solver | Simulated annealing for QUBO |
| Purge-group consistency term | Partial cache penalty |
| Origin-group burst shielding | Shared origin bonus |

### Data Requirements

| Level | Method | Data Source |
|-------|--------|-------------|
| V2.0 | Time-window co-occurrence | CDN logs only |
| V2.5 | Pseudo-session (IP+UA+gap) | CDN logs + heuristic |
| V3+ | App-level session ID | Application enrichment |

### Technical Design

```rust
struct PairwiseInteraction {
    i: u32,    // index into objects array
    j: u32,
    weight: f64,
}

struct QuadraticProblem {
    linear_terms: Vec<f64>,
    interactions: Vec<PairwiseInteraction>,
    sizes: Vec<u64>,
    capacity_bytes: u64,
}

trait QuadraticSolver {
    fn solve(&self, problem: &QuadraticProblem) -> QuadraticSolverResult;
}
```

### Scaling Strategy

1. Candidate pre-selection: top-K by linear benefit
2. Sparse interactions: co-access top neighbors only
3. Group-level optimization: origin/purge group granularity
4. Hybrid: linear preselection → quadratic refinement

---

## V2.5 — Provider Integration

**Goal:** Connect optimization results to CDN provider APIs

### Deliverables

- CloudFront: invalidation list, cache behavior rules
- Cloudflare: cache rules, bypass settings, purge targets
- Policy rollout/rollback mechanism
- Production vs replay KPI comparison (observability)
- Change management (churn control, diff limits)

---

## V3.0 — Quantum Backend (Experimental)

**Goal:** Connect QUBO to quantum hardware for research/demonstration

### Deliverables

- IBM Quantum adapter
- Amplify adapter
- Small-scale problem demonstration
- Classical vs quantum solver comparison paper

---

## Publication Timeline

| Phase | Blog/Paper | QUBO mention |
|-------|-----------|-------------|
| V1.0 | "CDN Cache Optimization as Economic Knapsack" | 最後に1段落だけ |
| V1.5 | "Trace Replay: LRU/GDSF/Belady比較" | なし |
| V2.0 | "Why QUBO: Quadratic Terms for Cache Co-access" | 主題 |
| V3.0 | "Classical vs Quantum QUBO for CDN Optimization" | 主題 |
