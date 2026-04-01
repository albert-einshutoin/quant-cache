# quant-cache Technology Stack

**Version:** 1.0
**Date:** 2026-03-31
**Status:** Confirmed

---

## 1. Language & Toolchain

- **Language:** Rust (stable)
- **Edition:** 2021
- **MSRV:** 1.75+
- **Build:** Cargo workspace

---

## 2. Dependencies by Crate

### qc-model

| Crate | Version | Purpose |
|-------|---------|---------|
| `serde` | 1.x | Serialize/Deserialize for all types |
| `serde_json` | 1.x | JSON output support |
| `chrono` | 0.4 | DateTime handling |
| `thiserror` | 2.x | Error type definitions |

### qc-solver

| Crate | Version | Purpose |
|-------|---------|---------|
| `serde` | 1.x | Serialization |
| `thiserror` | 2.x | Error types |
| `good_lp` | 1.x | ILP solver abstraction |
| `highs` | 1.x | HiGHS backend for ILP |
| `tracing` | 0.1 | Structured logging |

### qc-simulate

| Crate | Version | Purpose |
|-------|---------|---------|
| `serde` | 1.x | Serialization |
| `chrono` | 0.4 | Timestamp handling |
| `rand` | 0.8 | Random number generation |
| `rand_distr` | 0.4 | Statistical distributions (Zipf, LogNormal, Poisson) |
| `tracing` | 0.1 | Structured logging |
| `thiserror` | 2.x | Error types |

### qc-cli

| Crate | Version | Purpose |
|-------|---------|---------|
| `clap` | 4.x | CLI argument parsing |
| `serde` | 1.x | Serialization |
| `toml` | 0.8 | Config file parsing |
| `csv` | 1.x | CSV I/O |
| `tracing` | 0.1 | Structured logging |
| `tracing-subscriber` | 0.3 | Log subscriber |
| `anyhow` | 1.x | Error handling |

### Dev Dependencies (workspace-wide)

| Crate | Version | Purpose |
|-------|---------|---------|
| `proptest` | 1.x | Property-based testing (qc-solver, qc-simulate) |
| `criterion` | 0.5 | Benchmarking (qc-solver, qc-simulate) |

---

## 3. Configuration Format

- **Config:** TOML (human-readable, CLI-friendly)
- **Trace data:** CSV (primary), Parquet (optional, for large datasets)
- **Output:** JSON (structured), human-readable table (CLI default)

---

## 4. Error Handling Strategy

| Layer | Crate | Pattern |
|-------|-------|---------|
| Library crates | `thiserror` | 型付きエラー、`Result<T, E>` |
| CLI | `anyhow` | `anyhow::Result`, context chain |
| Boundary | — | Library → CLI 変換は `From` impl |

---

## 5. Logging Strategy

- `tracing` for structured, span-based logging
- Levels:
  - `ERROR`: 処理続行不可
  - `WARN`: 非推奨パス、fallback
  - `INFO`: solver 結果、比較サマリ
  - `DEBUG`: 個別オブジェクトスコア
  - `TRACE`: 内部ループ

---

## 6. CI Configuration

```yaml
# Minimum CI checks
- cargo fmt --check
- cargo clippy --all-targets -- -D warnings
- cargo test --workspace
- cargo test --workspace -- --ignored  # slow/integration tests
- cargo bench --no-run  # compile check only
```

---

## 7. Not Using (and Why)

| Crate | Reason |
|-------|--------|
| `polars` | 依存が重い。V1 は row-based I/O で十分 |
| `time` / `jiff` | chrono が Parquet/CSV 連携で最無難 |
| `argh` | clap が subcommand 構成で安定 |
| `fastrand` | 分布サポートが弱い |
| `log` / `env_logger` | tracing の方が拡張性高い |
| `coin_cbc` | 環境依存が面倒。good_lp + highs で十分 |
