# quant-cache Testing Strategy

**Version:** 2.0
**Date:** 2026-04-01
**Status:** Implemented

---

## 1. Test Summary

| Category | Count | Framework | Execution |
|----------|-------|-----------|-----------|
| Unit (qc-model) | 15 | `#[test]` | `cargo test` |
| Unit (qc-solver scoring) | 8 | `#[test]` | `cargo test` |
| Unit (qc-solver greedy) | 7 | `#[test]` | `cargo test` |
| Unit (qc-solver ILP) | 9 | `#[test]` | `cargo test` |
| Integration (replay) | 16 | `#[test]` | `cargo test` |
| Integration (synthetic) | 12 | `#[test]` | `cargo test` |
| Property-based (solver) | 5 | proptest | `cargo test` |
| Property-based (simulator) | 3 | proptest | `cargo test` |
| Acceptance (baselines) | 2 | `#[test] #[ignore]` | `cargo test --release -- --ignored` |
| Acceptance (gap 50 cases) | 1 | `#[test] #[ignore]` | `cargo test --release -- --ignored` |
| Performance guards | 2 | `#[test] #[ignore]` | `cargo test --release -- --ignored` |
| **Total** | **80** | | |

---

## 2. Critical Invariants (proptest)

### Solver Invariants (5 tests)

| ID | Invariant | Verified |
|----|-----------|----------|
| S1 | 容量制約を超えない | `greedy_respects_capacity` |
| S3 | `net_benefit <= 0` は greedy が拾わない | `greedy_excludes_negative_benefit` |
| S4 | 同一入力で deterministic | `greedy_is_deterministic` |
| S5 | 容量増加で objective 悪化しない | `greedy_monotone_in_capacity` |
| S6 | ILP 解 >= greedy 解 | `ilp_at_least_as_good_as_greedy` |

### Simulator Invariants (3 tests)

| ID | Invariant | Verified |
|----|-----------|----------|
| R1 | hit + miss = total requests | `hit_plus_miss_equals_total` |
| R2 | byte_hit_ratio ∈ [0, 1] | `byte_hit_ratio_in_range` |
| R5 | LRU never exceeds capacity | `lru_never_exceeds_capacity` |

---

## 3. Acceptance Criteria (measured results)

### Performance

| Criteria | Target | Actual |
|----------|--------|--------|
| Greedy 10k objects | < 1s | **0.6ms** |
| LRU replay 1M events | < 10s | **825ms** |

### Quality

| Criteria | Target | Actual |
|----------|--------|--------|
| Optimality gap median (n=1000, 50 cases) | < 5% | **0.01%** |
| Optimality gap p95 (n=1000, 50 cases) | < 10% | **0.72%** |
| EconomicGreedy beats LRU on cost savings | > 50% of cases | **16/20 (80%)** |

---

## 4. Test File Map

```text
crates/qc-model/tests/
  └── roundtrip.rs         # 15 serde round-trip tests

crates/qc-solver/tests/
  ├── scoring.rs           # 8 BenefitCalculator tests
  ├── greedy.rs            # 7 GreedySolver tests
  ├── ilp.rs               # 9 ExactIlpSolver tests
  ├── proptest_invariants.rs # 5 solver property tests
  ├── acceptance.rs        # 1 optimality gap (50 cases, ignored)
  └── perf_guard.rs        # 1 greedy 10k wall-clock guard (ignored)

crates/qc-simulate/tests/
  ├── replay.rs            # 16 replay/baseline/comparator tests
  ├── synthetic.rs         # 12 synthetic generator tests
  ├── proptest_invariants.rs # 3 simulator property tests
  ├── acceptance.rs        # 2 baseline comparison tests (ignored)
  └── perf_guard.rs        # 1 LRU 1M wall-clock guard (ignored)

crates/qc-solver/benches/
  └── greedy_solver.rs     # criterion: greedy 10k benchmark

crates/qc-simulate/benches/
  └── trace_replay.rs      # criterion: LRU/GDSF 1M replay benchmarks
```

---

## 5. Running Tests

```bash
# All regular tests (75 tests)
cargo test --workspace

# Acceptance + performance guards (5 tests, release build)
cargo test --release --workspace -- --ignored

# Benchmarks
cargo bench -p qc-solver
cargo bench -p qc-simulate

# Clippy + fmt
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

---

## 6. Coverage

Tooling: `cargo-llvm-cov` (LLVM source-based coverage)

Target:
- Line coverage >= 80%
- Critical paths (solver, scoring, replay) >= 90%
