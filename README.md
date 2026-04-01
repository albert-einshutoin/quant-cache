# quant-cache

Economic CDN cache optimization engine powered by constrained optimization.

quant-cache selects which objects to cache by solving a **0-1 knapsack problem**
that maximizes expected economic benefit (latency savings + origin cost reduction)
under cache capacity and freshness constraints. Unlike heuristic-based policies
(LRU, GDSF), it formulates cache admission as an explicit optimization problem
with a well-defined objective function in $/period.

## Quick Start

```bash
# Build
cargo build --workspace --release

# 1. Generate a synthetic trace (10k objects, 1M requests)
qc generate --num-objects 10000 --num-requests 1000000 --output trace.csv

# 2. Optimize cache policy
qc optimize --input trace.csv --output policy.json \
  --capacity 50000000 --preset ecommerce

# 3. Replay trace against the optimized policy
qc simulate --input trace.csv --policy policy.json

# 4. Compare against LRU and GDSF baselines
qc compare --input trace.csv --capacity 50000000 --preset ecommerce

# 5. Include ILP for optimality gap measurement (slow for large n)
qc compare --input trace.csv --capacity 50000000 --preset ecommerce --include-ilp
```

Detailed usage:

- [docs/usage-guide.md](/Users/shutoide/Developer/quant-cache/docs/usage-guide.md)

## How It Works

```
Trace (CSV) → aggregate → ObjectFeatures
                              │
                    BenefitCalculator (score.rs)
                              │
                        ScoredObject[]
                              │
               ┌──────────────┴──────────────┐
          GreedySolver                  ExactIlpSolver
          O(n log n)                    Exact (n < 10k)
               │                              │
          PolicyDecision[]               PolicyDecision[]
               │                              │
          TraceReplayEngine ◄─────────────────┘
               │
          MetricsSummary (hit ratio, cost savings, objective value, stale rate)
```

### Scoring Formula

For each object *i* over time window *T*:

```
benefit_i = E[requests_i] × (latency_saving_i × λ_latency + origin_cost_i)

freshness_cost_i =
  TTL-Only:    E[requests_i] × (1 - e^(-update_rate_i × ttl_i)) × stale_penalty_i
  Invalidation: update_rate_i × T × invalidation_cost

net_benefit_i = benefit_i - freshness_cost_i
```

The solver maximizes `Σ net_benefit_i × x_i` subject to `Σ size_i × x_i ≤ capacity`.

## Architecture

```
quant-cache/
├── crates/
│   ├── qc-model/        Data types, configs, presets, error types
│   ├── qc-solver/       BenefitCalculator, GreedySolver, ExactIlpSolver
│   ├── qc-simulate/     TraceReplayEngine, LRU/GDSF baselines, synthetic generator
│   └── qc-cli/          CLI (generate, optimize, simulate, compare)
├── data/
│   ├── samples/         Sample trace CSV and TOML config
│   └── schemas/         Trace event schema definition
└── docs/                Design documents, strategy, related work
```

## CLI Commands

### `qc generate`

Generate synthetic trace data with configurable distributions.

```bash
qc generate --num-objects 10000 --num-requests 1000000 \
  --zipf-alpha 0.8 --seed 42 --output trace.csv
```

### `qc optimize`

Score objects and solve the knapsack to produce a cache policy.

```bash
# Using a preset
qc optimize --input trace.csv --output policy.json --capacity 50000000 --preset ecommerce

# Using a TOML config file
qc optimize --input trace.csv --output policy.json --config scenario.toml

# Using ILP solver for exact solution
qc optimize --input trace.csv --output policy.json --capacity 50000000 --preset ecommerce --ilp
```

### `qc simulate`

Replay a trace against a saved policy and report metrics.

```bash
qc simulate --input trace.csv --policy policy.json --output metrics.json
```

### `qc compare`

Compare EconomicGreedy against LRU and GDSF baselines on the same trace.

```bash
qc compare --input trace.csv --capacity 50000000 --preset ecommerce
qc compare --input trace.csv --capacity 50000000 --preset ecommerce --include-ilp
```

Output:

```
Policy                     Hit%     ByteHit%   CostSavings$     Objective$
---------------------------------------------------------------------------
LRU                      50.38%       42.82%       1511.3610       1284.2800
GDSF                     72.53%       43.47%       2175.9540       1205.3400
EconomicGreedy           72.45%       17.89%       2173.6410       1397.2500
```

## Presets

| Preset | Use Case | λ_latency ($/ms) | Stale Penalty |
|--------|----------|-------------------|---------------|
| `ecommerce` | Product pages, catalogs | 0.00005 | High ($0.10/event) |
| `media` | Video/image streaming | 0.00001 | Low ($0.001/event) |
| `api` | REST APIs, auth tokens | 0.0001 | InvalidationOnUpdate ($0.001/event) |

Stale penalty costs are configurable via `StaleCostOverrides` in TOML config.

## TOML Config Example

```toml
capacity_bytes = 50_000_000
time_window_seconds = 86400
latency_value_per_ms = 0.00005

[freshness_model]
type = "TtlOnly"

[freshness_model.stale_penalty]
default_class = "high"

[freshness_model.stale_penalty.cost_overrides]
high = 0.2    # override default $0.10 to $0.20
medium = 0.05
```

## Solvers

| Solver | Use | Complexity | Notes |
|--------|-----|------------|-------|
| GreedySolver | Default | O(n log n) | Runs ratio + pure-benefit variants, picks best |
| ExactIlpSolver | Verification | Exact | HiGHS backend, practical for n < 10,000 |

### V1 Performance

| Metric | Result | Target |
|--------|--------|--------|
| Greedy 10k objects | 0.6ms | < 1s |
| Trace replay 1M events | 825ms | < 10s |
| Optimality gap (median, n=1000) | 0.01% | < 5% |
| Optimality gap (p95, n=1000) | 0.72% | < 10% |

## Freshness Models

V1 provides two mutually exclusive models to prevent double-counting:

- **TTL-Only**: No active invalidation. Stale penalty accrues when cached content
  exceeds TTL or version changes. Uses Poisson model: `P(stale) = 1 - e^(-λt)`.
- **InvalidationOnUpdate**: Every content update triggers a purge. No stale serving.
  Cost = `update_rate × T × cost_per_invalidation`.

## Testing

```bash
cargo test --workspace                                    # 75 unit/integration/proptest tests
cargo test --release --workspace -- --ignored             # acceptance + performance guards
cargo bench -p qc-solver                                  # greedy solver benchmarks
cargo bench -p qc-simulate                                # replay benchmarks
```

### Test Categories

| Category | Count | Framework |
|----------|-------|-----------|
| Unit tests | 15 (model) + 8 (scoring) + 7 (greedy) + 9 (ILP) | `#[test]` |
| Integration | 16 (replay) + 12 (synthetic) | `#[test]` |
| Property-based | 5 (solver) + 3 (simulator) | proptest |
| Acceptance | 2 (baseline comparison) + 1 (optimality gap 50 cases) | `#[test] #[ignore]` |
| Performance guards | 2 (greedy 10k, LRU 1M) | `#[test] #[ignore]` |

## Vision

quant-cache approaches CDN caching as a **formal optimization problem**, not a heuristic.

Traditional cache policies (LRU, LFU, GDSF) make local, greedy decisions about what to evict.
quant-cache asks a different question: *given full knowledge of your traffic patterns,
what is the economically optimal set of objects to cache?*

V1 solves this as a 0-1 knapsack. Future versions will introduce **quadratic interactions**
(QUBO formulation) to model co-access patterns, purge-group consistency, and origin-group
shielding — optimizations that have no equivalent in classical cache heuristics.

> **Current status:** V1 is validated on synthetic traces.
> Real CDN trace validation (CloudFront/Cloudflare) is the next milestone.

## Roadmap

| Version | Focus |
|---------|-------|
| **V1.0** (current) | Economic knapsack + trace replay evaluation |
| V1.1 | CloudFront/Cloudflare log import + real trace validation |
| V1.5 | Belady baseline, offline coefficient calibration |
| V2.0 | QUBO with quadratic terms (co-access, purge-group, origin-group) |
| V2.5 | CDN provider API integration |
| V3.0 | Quantum backend experiments |

See [docs/roadmap.md](docs/roadmap.md) for the detailed plan.

## Documentation

- [Architecture](docs/ARCHITECTURE.md) — Crate responsibilities, data flow, design decisions
- [Formulation](docs/formulation.md) — Mathematical formulation (knapsack, Lagrangian, QUBO preview)
- [Roadmap](docs/roadmap.md) — V1.0 → V3.0 detailed plan
- [Related Work](docs/related-work.md) — 17 papers surveyed, positioning vs GDSF/ARC/ML
- [V1 Design](docs/v1-design.md) — Data model, acceptance criteria, risk analysis
- [Tech Stack](docs/tech-stack.md) — Rust crate dependencies
- [Testing Strategy](docs/testing-strategy.md) — Test categories, critical invariants

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

[MIT](LICENSE)
