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
  done       done        done        done        planned
evaluation  Policy IR   Policy      Deployment  Multi-vendor
framework   + evaluator search      scaffold    + quantum
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

### Future Targets

- Fastly VCL / Compute
- CloudFront Functions / Lambda@Edge
- Akamai Property Manager / EdgeWorkers

---

## Phase E — Validation, cache_key_rules, Multi-Vendor

**Goal:** Deployability hardening, cache key normalization, additional providers

### cache_key_rules Implementation

PolicyIR の `cache_key_rules` は定義済みだが評価・探索ともに未実装。
実装には `regex` crate の依存追加が必要。

```rust
// 例: UTM パラメータを除去して cache key を正規化
{ "pattern": "[?&]utm_[^&]*", "replacement": "" }
```

用途:
- Query parameter stripping (UTM, session ID)
- Device variant normalization (/mobile/ → /)
- Cloudflare Cache Rules の Cache Key 設定に直接マップ可能

### Deliverables

- cache_key_rules の IR evaluator 実装 (regex 依存)
- policy-search での cache_key_rules 探索
- Cloudflare / CloudFront API schema validation
- Cross-CDN policy comparison (same IR, different targets)
- Fastly VCL / Compute target
- IBM Quantum / Amplify adapter for policy search (research)

---

## Publication Timeline

| Phase | Blog/Paper | Focus |
|-------|-----------|-------|
| A | "GDSF Scores Negative: Why Hit Rate Isn't Enough" | Economic evaluation finding |
| B | "Policy IR: A Unified Language for Cache Configuration" | DSL design |
| C | "Quantum-Inspired Policy Search for CDN Caching" | SA/QUBO over DSL |
| D | "From Trace to Cloudflare Rules: Automated Cache Configuration" | Vendor compiler |
