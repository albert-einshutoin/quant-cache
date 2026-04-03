# quant-cache

**English** | [日本語](README.ja.md)

An economic cache decision framework for CDN operators.

quant-cache evaluates cache policies through an **economic objective function** ($/period)
that unifies latency savings, origin cost reduction, and freshness penalties into a single
metric. It reveals hidden costs that hit-rate-only evaluation misses — for example, GDSF
achieves the highest hit rate but scores **negative on economic objective** due to stale
content penalties.

quant-cache is not a replacement for eviction policies like SIEVE or S3-FIFO.
It is a **decision and evaluation layer** that answers:
- Which objects are economically worth caching?
- How does your cache policy perform when freshness costs are accounted for?
- How close is your greedy heuristic to the mathematical optimum?

## Key Finding

Evaluated across 20 synthetic traces (Zipf α=0.6, 500 objects, 50k requests):

| Policy | Objective$ (mean) | Hit% (mean) | CostSavings$ (mean) |
|--------|-------------------|-------------|---------------------|
| **SIEVE** | **392.57** | 36.4% | 361.74 |
| S3-FIFO | 380.48 | 35.1% | 350.79 |
| LRU | 324.90 | 35.0% | 349.66 |
| **GDSF** | **-133.19** | **44.1%** | **562.64** |

GDSF has the highest hit rate and cost savings, but its **economic objective is deeply
negative** because it caches high-update-rate objects that incur stale penalties.
This kind of insight is invisible without an explicit economic model.

## Quick Start

```bash
cargo build --workspace --release

# Generate a synthetic trace
qc generate --num-objects 10000 --num-requests 1000000 --output trace.csv

# Compare policies with economic evaluation
qc compare --input trace.csv --capacity 50000000 --preset ecommerce

# Import real CloudFront logs
qc import --provider cloudfront --input access.log --output trace.csv

# Optimize: find the economically optimal cache set
qc optimize --input trace.csv --output policy.json --capacity 50000000 --preset ecommerce

# Calibrate economic parameters
qc calibrate --train train.csv --validation val.csv --capacity 50000000
```

## End-to-End: Trace → Cloudflare Config

```bash
# 1. Import CloudFront logs (or generate synthetic)
qc import --provider cloudfront --input access.log --output trace.csv

# 2. Search for the best policy configuration
qc policy-search --input trace.csv --capacity 50000000 \
  --preset ecommerce --output best-policy.json

# 3. Evaluate the policy on the trace
qc policy-eval --input trace.csv --policy best-policy.json --preset ecommerce

# 4. Generate optimizer scores for admission gate
qc optimize --input trace.csv --output scores.json \
  --capacity 50000000 --preset ecommerce

# 5. Compile to Cloudflare Rulesets API payload
qc compile --policy best-policy.json --scores scores.json \
  --target cloudflare --output cloudflare-config.json

# 6. (Or compile to CloudFront)
qc compile --policy best-policy.json --scores scores.json \
  --target cloudfront --output cloudfront-config.json
```

The output `cloudflare-config.json` contains:
- Rulesets API payload (`http_request_cache_settings` phase)
- Workers script with populated admission scores
- Prewarm URL list
- Step-by-step deploy instructions

## What It Does

### 1. Economic Scoring

For each cached object, compute expected economic benefit over time window T:

```
benefit  = E[requests] × (latency_saving × λ_latency + origin_cost)
freshness_cost = E[requests] × P(stale) × stale_penalty    (TTL-Only model)
net_benefit = benefit - freshness_cost
```

All terms are in $/period. This makes cost/benefit comparable across objects with
different sizes, access patterns, and update frequencies.

### 2. Replay Evaluation

Replay traces against multiple policies (LRU, GDSF, SIEVE, S3-FIFO, Belady) and
measure both traditional metrics (hit rate, byte hit rate) and economic objective
(incorporating latency value and per-object stale penalties).

### 3. Bounded Optimality

Solve the 0-1 knapsack with GreedySolver (O(n log n)) and verify against ExactIlpSolver.
Observed optimality gap: **median 0.01%, p95 0.72%** (n=1000, 50 cases).

## CLI Commands

| Command | Description |
|---------|-------------|
| `qc import` | Convert CDN provider logs (CloudFront) to canonical trace CSV |
| `qc generate` | Generate synthetic traces with configurable distributions |
| `qc optimize` | Find economically optimal cache set (greedy, ILP, or SA solver) |
| `qc simulate` | Replay trace against a saved policy |
| `qc compare` | Compare LRU, GDSF, SIEVE, S3-FIFO side-by-side with economic metrics |
| `qc calibrate` | Auto-tune economic parameters using train/validation split |
| `qc policy-eval` | Evaluate PolicyIR configurations on traces |
| `qc policy-search` | Search backend/admission/bypass/prewarm space for best config |
| `qc compile` | Generate deployment scaffold (Cloudflare Cache Rules + Worker) |

## Baselines

| Policy | Type | Source |
|--------|------|--------|
| LRU | Online eviction | Classic |
| GDSF | Online eviction (cost-aware) | Cao & Irani, 1997 |
| SIEVE | Online eviction (lazy promotion) | Zhang et al., NSDI 2024 (Best Paper) |
| S3-FIFO | Online eviction (3-queue) | Yang et al., SOSP 2023 |
| Belady | Offline oracle (future knowledge) | Belady, 1966 |
| EconomicGreedy | Offline knapsack selection | quant-cache (Dantzig, 1957) |
| ExactILP | Offline optimal | HiGHS solver |

## Presets

| Preset | Use Case | λ_latency ($/ms) | Stale Penalty |
|--------|----------|-------------------|---------------|
| `ecommerce` | Product pages, catalogs | 0.00005 | High ($0.10/event) |
| `media` | Video/image streaming | 0.00001 | Low ($0.001/event) |
| `api` | REST APIs, auth tokens | 0.0001 | InvalidationOnUpdate |

Stale penalty costs are configurable per class via `StaleCostOverrides` in TOML config.

## Architecture

```
quant-cache/
├── crates/
│   ├── qc-model/      Data types, configs, presets, economic parameters
│   ├── qc-solver/     BenefitCalculator, GreedySolver, ExactIlpSolver, SA solver, calibration
│   ├── qc-simulate/   Replay engine, 5 baseline policies, synthetic generator, co-access
│   └── qc-cli/        CLI (10 commands: import → generate → optimize → policy-search → policy-eval → compile)
├── data/samples/      Sample traces and configs
└── docs/              Design documents, related work (29 papers)
```

## Academic Context

quant-cache is grounded in 29 surveyed papers spanning classical algorithms (Belady, GDSF),
modern eviction (SIEVE, S3-FIFO, TinyLFU), ML approaches (LRB, CACHEUS), optimization
theory (Dantzig knapsack, Lucas QUBO), and production systems (CacheLib).

See [docs/related-work.md](docs/related-work.md) for the full survey.

## Roadmap

| Version | Focus | Status |
|---------|-------|--------|
| V1.0 | Economic knapsack + trace replay | Done |
| V1.1 | CloudFront log import | Done |
| V1.5 | Belady oracle, calibration | Done |
| V2.0 | Quadratic SA, co-access | Done |
| Phase B | Policy IR + IR evaluator | Done |
| Phase C | Policy search (all PolicyIR fields) | Done |
| Phase D | Cloudflare + CloudFront deployment scaffolds | Done |
| Phase E | Provider schema validation, quantum search | Planned |

## Testing

```bash
cargo test --workspace                                # 80+ tests
cargo test --release --workspace -- --ignored         # acceptance + perf guards
cargo clippy --all-targets -- -D warnings             # lint
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
