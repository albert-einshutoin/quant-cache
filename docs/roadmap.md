# quant-cache Roadmap

**Version:** 2.0
**Date:** 2026-04-02
**Status:** Revised — Economic Cache Control Plane direction

---

## Strategic Direction

quant-cache evolves from an evaluation framework into an **economic cache control plane**:
- Evaluate cache policies through explicit economic objectives
- Search the policy design space using quantum-inspired optimization (design-time only)
- Generate vendor-native cache configurations for deployment

## Phase Overview

```text
Phase A ──→ Phase B ──→ Phase C ──→ Phase D ──→ Phase E
  done       done        done        done        done*
evaluation  Policy IR   Policy      Deployment  Multi-vendor
framework   + evaluator search      scaffold    + quantum

V1.6 Reuse Distance Scoring: DONE
(*Phase E: real deploy validation remains — requires production environment)
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

## V1.6 — Reuse Distance Scoring (DONE)

**Goal:** Replace frequency-based demand estimation with reuse-distance-aware scoring

### Deliverables

| Item | Description | Status |
|------|-------------|--------|
| Reuse distance computation | Per cache_key reuse distance distribution from trace | Done |
| BenefitCalculatorV2 | Hit probability via `exp(-rd_p50 / cache_capacity_objects)` | Done |
| ObjectFeatures extension | mean_reuse_distance, reuse_distance_p50, reuse_distance_p95 | Done |
| aggregate_features integration | Reuse distance auto-populated from trace | Done |
| ScoringVersion config | `scoring_version` field in ScenarioConfig (v1_frequency / v2_reuse_distance) | Done |
| CLI `--scoring v2` flag | `qc optimize --scoring v2` for reuse-distance scoring | Done |
| A/B comparison | V1 vs V2 scoring tests on synthetic traces (16 tests) | Done |

### Hit Probability Model

```
p_hit = exp(-reuse_distance_p50 / cache_capacity_objects)
```

- `cache_capacity_objects = capacity_bytes / object_size_bytes`
- Low reuse distance (high locality) → p_hit ≈ 1.0 (near V1)
- High reuse distance (low locality) → p_hit → 0 (significantly discounted)
- Falls back to V1 when reuse distance data is unavailable

### Academic Basis

- Paper 15: PRP (Probabilistic Replacement Policy)
- Paper 16: Reuse Distance & Stream Detection (Keramidas, 2007)

---

## V2.0 — QUBO with Quadratic Terms (DONE)

**Goal:** Introduce pairwise interactions for co-access, purge-group, origin-group

### Deliverables

| Item | Description | Status |
|------|-------------|--------|
| QuadraticProblem type | linear_terms + sparse PairwiseInteraction list | Done |
| QuadraticSolver trait | Separate from linear Solver trait | Done |
| Co-access extraction | Time-window co-occurrence counting from traces | Done |
| SA solver | Simulated annealing for QUBO | Done |
| Purge-group consistency term | Co-caching bonus for same purge group (`--purge-group-weight`) | Done |
| Origin-group burst shielding | Co-caching bonus for same origin (`--origin-group-weight`) | Done |
| Synthetic group generation | `num_purge_groups` / `num_origin_groups` in SyntheticConfig | Done |
| Interaction composition | Co-access + purge + origin merged in SA solver | Done |

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

---

## Phase B — Policy IR + Evaluator (DONE)

**Goal:** Define a policy intermediate representation and replay it

### Policy IR

```rust
struct PolicyIR {
    backend: Backend,              // SIEVE | S3FIFO | TinyLFU
    admission_rule: AdmissionRule, // always | score > τ | score/size > τ
    bypass_rule: BypassRule,       // freshness_risk > τ | size > τ
    prewarm_set: Vec<String>,      // top-k by objective
    ttl_class_rules: Vec<TtlClassRule>,
    cache_key_rules: Vec<CacheKeyRule>,
}
```

### Deliverables

- Policy IR type definitions in qc-model
- IR-based replay in qc-simulate (not just CachePolicy trait)
- `qc policy-eval` CLI command: evaluate IR configs on traces
- Comparison of IR configurations vs pure baselines

---

## Phase C — Policy Search Engine (DONE)

**Goal:** Search the policy configuration space for optimal settings

### Current Status

`qc policy-search` searches over:
- Backend (SIEVE / S3-FIFO)
- Admission rule (Always / ScoreThreshold / ScoreDensityThreshold)
- Bypass rule (None / SizeLimit / FreshnessRisk / composite Any)
- Prewarm set (top-k by score)
- TTL class rules (content-type prefix → TTL from trace-observed types)

**Fully searched:**
- backend, admission, bypass (composite), prewarm, TTL class rules, cache_key_rules

### Remaining Work

- SA/QUBO over the full discrete policy configuration space

### Quantum-Inspired Role

QUBO/SA searches over the policy DSL space, not individual object selection.
This is where quadratic interactions (co-access, purge-group) become useful:
they inform which policy configurations handle correlated access patterns.

---

## Phase D — Deployment Scaffold Generator (DONE)

**Goal:** Generate cache configuration scaffolds for CDN providers

### Current Status

`qc compile --target cloudflare` generates a **deployment scaffold**:
- Cloudflare Cache Rules for bypass (size limit) and TTL overrides
- Cloudflare Workers script template for admission gate
- Prewarm URL list
- Backend recommendation note

**Current capabilities:**
- Worker `ADMISSION_SCORES` populated via `qc compile --scores policy.json` (reads PolicyFile from `qc optimize`)
- FreshnessRisk bypass maps to content-type-based Cloudflare expression
- Composite bypass rules (BypassRule::Any) fully compiled
- TTL class rules compiled to Cloudflare cache TTL expressions

**Current capabilities:**
- Cloudflare output uses Rulesets API format (http_request_cache_settings phase)
- CloudFront output generates CacheBehaviors with standard CachePolicyIds
- Both targets support --scores for populated admission gate code
- Deploy steps documented in output JSON

**Remaining limitations:**
- Backend choice is advisory (CDN providers don't expose eviction algo selection)
- Manual review before deployment still recommended

### Remaining Work

- Add --target fastly (VCL/Compute)
- Cloudflare API validation / direct deployment
- Validate generated config against Cloudflare API schema

### Supported Targets

- Cloudflare (Rulesets API + Workers)
- CloudFront (CacheBehaviors + CloudFront Functions)
- Fastly (VCL snippets + Edge Dictionary)
- Akamai (Property Manager rule tree + EdgeWorkers)

### Future Targets

- Lambda@Edge (CloudFront advanced)
- Varnish (open-source VCL)

---

## Phase E — Parametric Validation + Multi-Vendor (REVISED)

**Goal:** Systematic parametric validation, provider schema validation, quantum-inspired DSL search

> **History (2026-04-06): Phase E goal revised.**
> Original goal was "real deploy validation" — deploying to a production CloudFront
> environment and measuring before/after cost. This was replaced with a parametric
> validation suite for the following reasons:
>
> 1. A single production trace is N=1 — it only covers one Zipf distribution, one
>    object size profile, one update rate. It cannot validate edge cases.
> 2. At individual/small-team scale, production traffic is statistically thin.
>    Too few requests for meaningful significance testing.
> 3. Synthetic data with controlled parameters can systematically sweep the full
>    parameter space (2,880+ combinations), covering scenarios that production
>    traces may never encounter.
> 4. Automated invariant assertions (Belady ceiling, capacity bounds, NaN guards,
>    monotonicity) are more rigorous than "it worked in prod once."
> 5. The suite is reproducible — anyone can run it. Production validation is
>    environment-dependent and non-reproducible.
>
> Production replay is retained as optional Phase F for those who have real CDN logs.

### Deliverables

1. **Provider schema validation** — DONE
   - `qc compile --validate` flag for all 4 targets

2. **Parametric validation suite** (replaces "real deploy validation")
   - Parameter sweep: Zipf α × capacity ratio × object count × update rate × scoring × solver
   - Three tiers: smoke (default, <30s), CI (release, <5min), full (ignored, <60min)
   - Invariants: Belady ceiling, objective ordering, capacity, NaN/Inf, monotonicity, determinism
   - Object size distribution + freshness model sub-sweeps

3. **Fastly VCL target** — DONE
   - `qc compile --target fastly`

4. **Cross-CDN comparison** — DONE
   - `qc compile-compare` command

5. **Quantum-inspired policy search**
   - SA/QUBO over the full discrete PolicyIR configuration space
   - Replace grid+random with structured annealing over DSL

### Validation Tiers

| Tier | When | Combos | Runtime | Gate |
|------|------|--------|---------|------|
| Smoke | `cargo test` | ~30 | <30s | Every commit |
| CI | `cargo test --release` | ~200 | <5min | PR merge |
| Full | `cargo test --release -- --ignored` | ~2,880 | <60min | Release |

---

## Phase F — Production Replay (Optional)

**Goal:** Validate on real CDN logs for those with production environments.

This is not a gate for project completion. It is an optional demonstration that
the framework produces meaningful results on real traffic patterns.

- End-to-end: real CloudFront trace → policy-search → compile → deploy → measure
- Before/after cost comparison on production traffic
- Document results as a case study

---

## Decision Log

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-04-02 | V2.0 roadmap: evolve from eval framework to economic cache control plane | Phases A-E complete, strategic direction established |
| 2026-04-05 | 5-agent review → P0-P3 remediation cycle | Architecture/Rust/Security/Test/Codex review identified CRITICAL/HIGH issues in baselines, compile, scoring |
| 2026-04-06 | Gitflow adoption (develop/main) | Growing commit volume needs structured branching for review cycles |
| 2026-04-06 | Phase E goal revised: "real deploy validation" → "parametric validation suite" | N=1 production trace is unfalsifiable; parametric sweep covers 2,880+ scenarios systematically |
| 2026-04-06 | Production replay demoted to optional Phase F | Individual-scale production traffic is statistically thin; parametric tests are more rigorous |
| 2026-04-06 | Scorer trait introduced (V1Scorer/V2Scorer) | Enables clean extension for future scoring versions without modifying BenefitCalculator |
| 2026-04-06 | SolverResult/QuadraticResult unified | Eliminates 5-tuple destructuring in CLI; single result type for all solvers |
| 2026-04-06 | O(n²) algorithms capped | reuse_distance (100K gap), co_access (50M pairs), group_interactions (sqrt pre-selection) |

---

## Publication Timeline

| Phase | Blog/Paper | Focus |
|-------|-----------|-------|
| A | "GDSF Scores Negative: Why Hit Rate Isn't Enough" | Economic evaluation finding |
| B | "Policy IR: A Unified Language for Cache Configuration" | DSL design |
| C | "Quantum-Inspired Policy Search for CDN Caching" | SA/QUBO over DSL |
| D | "From Trace to Cloudflare Rules: Automated Cache Configuration" | Vendor compiler |
| E | "2,880 Scenarios: Parametric Validation for Cache Economics" | Validation methodology |
