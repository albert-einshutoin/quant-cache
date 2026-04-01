# quant-cache — Project Guide for Claude Code

## What This Is

CDN cache optimization engine. Formulates cache admission as a 0-1 knapsack
problem with economic objective ($/period). Rust workspace with 4 crates.

## Build & Test

```bash
cargo test --workspace                          # 75 regular tests
cargo test --release --workspace -- --ignored   # 5 acceptance + perf guards
cargo clippy --all-targets -- -D warnings       # must be clean
cargo fmt --check                               # must pass
```

## Architecture

```
qc-model  →  qc-solver  →  qc-simulate  →  qc-cli
(types)      (scoring+     (replay+         (CLI
              solving)      baselines)       interface)
```

Scoring and solving are separated:
- `BenefitCalculator` transforms `ObjectFeatures → ScoredObject`
- `Solver` trait receives `ScoredObject[]` + `CapacityConstraint`
- Solver never sees economic parameters directly

## Key Files

- `crates/qc-solver/src/score.rs` — scoring formula (the economic core)
- `crates/qc-solver/src/greedy.rs` — greedy solver (dual ratio + pure-benefit)
- `crates/qc-simulate/src/engine.rs` — trace replay with per-object stale penalty
- `crates/qc-simulate/src/baselines.rs` — LRU, GDSF, StaticPolicy with stale detection
- `crates/qc-simulate/src/synthetic.rs` — trace generator + feature aggregation
- `crates/qc-model/src/scenario.rs` — config types (FreshnessModel, StaleCostOverrides)

## Important Design Decisions

- `FreshnessModel` is an enum (TTL-Only XOR InvalidationOnUpdate) to prevent double-counting
- Stale penalty costs are per-object via `StalePenaltyClass`, overridable via `StaleCostOverrides`
- `PolicyFile` JSON includes `SolverMetadata` for `simulate` to restore diagnostics
- EconomicGreedy is static/offline — structurally different from online LRU/GDSF
- Trace events carry `version_or_etag` for version-mismatch stale detection

## Conventions

- Error handling: `thiserror` in libraries, `anyhow` in CLI
- Logging: `tracing` crate
- Config format: TOML
- Test data: synthetic generator, not checked-in large files
- Ignored tests: acceptance + perf guards, run with `--release --ignored`
