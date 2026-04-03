# quant-cache — Project Guide for Claude Code

## What This Is

Economic cache decision framework for CDN operators. Evaluates cache policies
through an economic objective ($/period) that unifies latency, origin cost,
and freshness penalties. **Not** a replacement for eviction policies (SIEVE, S3-FIFO) —
it is a decision/evaluation layer and admission policy foundation.

Key insight: GDSF achieves highest hit rate but scores **negative on economic
objective** due to stale penalties. This is invisible without explicit economic modeling.

## Build & Test

```bash
cargo test --workspace                          # 80+ regular tests
cargo test --release --workspace -- --ignored   # acceptance + perf guards
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

- `crates/qc-solver/src/score.rs` — economic scoring formula (the core value proposition)
- `crates/qc-solver/src/greedy.rs` — greedy knapsack solver
- `crates/qc-solver/src/qubo.rs` — quadratic SA solver (co-access interactions)
- `crates/qc-solver/src/calibrate.rs` — coefficient calibration (coordinate descent)
- `crates/qc-simulate/src/engine.rs` — trace replay with per-object economic evaluation
- `crates/qc-simulate/src/baselines.rs` — LRU, GDSF, SIEVE, S3-FIFO, Belady, EconAdmission hybrids
- `crates/qc-simulate/src/synthetic.rs` — trace generator (Zipf α default: 0.6)
- `crates/qc-simulate/src/co_access.rs` — co-occurrence extraction for quadratic terms
- `crates/qc-cli/src/providers/cloudfront.rs` — CloudFront log parser
- `crates/qc-cli/src/commands/compile.rs` — Cloudflare/CloudFront compiler + --validate
- `crates/qc-cli/src/commands/deploy_check.rs` — Pre-deploy safety gate

## Important Design Decisions

- quant-cache is an **evaluation framework and admission optimizer**, not an eviction policy
- EconomicGreedy (static offline) is a reference optimizer, not the primary product
- SIEVE/S3-FIFO are the runtime eviction baselines (2023-2024 SOTA)
- `FreshnessModel` enum prevents stale/invalidation double-counting
- Per-object `StalePenaltyClass` with configurable overrides
- `ReplayEconConfig` mirrors solver objective in replay for fair comparison
- Zipf α=0.6 default based on CacheLib production findings (α=0.3-0.7)

## Conventions

- Error handling: `thiserror` in libraries, `anyhow` in CLI
- Logging: `tracing` crate
- Config format: TOML
- Test data: synthetic generator, not checked-in large files
- Ignored tests: acceptance + perf guards, run with `--release --ignored`
