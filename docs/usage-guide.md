# quant-cache Usage Guide

## Overview

`quant-cache` は、トレースをもとに「どのオブジェクトをキャッシュすると経済的に得か」を計算し、
その結果を replay で検証する CLI ツールです。

V0.1.0 でできること:

- synthetic trace の生成
- trace から cache policy の最適化
- 保存済み policy の replay
- `EconomicGreedy` と `LRU` / `GDSF` の比較
- optional な `ILP` による解品質確認

## Prerequisites

- Rust toolchain
- `cargo build --workspace --release` が通る環境

CLI バイナリは次のどちらでも実行できます。

```bash
cargo run --release -p qc-cli -- <subcommand> ...
```

```bash
./target/release/qc <subcommand> ...
```

## Quick Workflow

最短の利用フローは次の 4 ステップです。

### 1. Trace を用意する

まず synthetic trace を生成します。

```bash
cargo run --release -p qc-cli -- generate \
  --num-objects 10000 \
  --num-requests 1000000 \
  --zipf-alpha 0.8 \
  --seed 42 \
  --output trace.csv
```

手元確認だけなら、既存の sample も使えます。

- [small.csv](/Users/shutoide/Developer/quant-cache/data/samples/small.csv)

trace schema は以下です。

- [trace-event-v1.toml](/Users/shutoide/Developer/quant-cache/data/schemas/trace-event-v1.toml)

### 2. Policy を最適化する

preset を使う場合:

```bash
cargo run --release -p qc-cli -- optimize \
  --input trace.csv \
  --output policy.json \
  --capacity 50000000 \
  --preset ecommerce
```

TOML config を使う場合:

```bash
cargo run --release -p qc-cli -- optimize \
  --input trace.csv \
  --output policy.json \
  --config data/samples/ecommerce.toml
```

利用可能な sample config:

- [ecommerce.toml](/Users/shutoide/Developer/quant-cache/data/samples/ecommerce.toml)

このコマンドで出る `policy.json` には以下が含まれます。

- solver metadata
- cached / not cached の decision 一覧
- 各 object の score breakdown

### 3. Policy を replay する

```bash
cargo run --release -p qc-cli -- simulate \
  --input trace.csv \
  --policy policy.json \
  --output metrics.json
```

主な出力:

- `hit_ratio`
- `byte_hit_ratio`
- `estimated_cost_savings`
- `policy_objective_value`
- `stale_serve_rate`
- `capacity_utilization`
- `solve_time_ms`
- `optimality_gap`（ILP を使った場合のみ）

### 4. Baseline と比較する

```bash
cargo run --release -p qc-cli -- compare \
  --input trace.csv \
  --capacity 50000000 \
  --preset ecommerce
```

ILP も比較に含める場合:

```bash
cargo run --release -p qc-cli -- compare \
  --input trace.csv \
  --capacity 50000000 \
  --preset ecommerce \
  --include-ilp
```

この比較では次を並べて見られます。

- `LRU`
- `GDSF`
- `EconomicGreedy`
- `ExactILP`（optional）

## Inputs and Outputs

### Input: Trace CSV

最低限、次の列が重要です。

- `timestamp`
- `cache_key`
- `object_size_bytes`
- `origin_fetch_cost`
- `response_latency_ms`
- `eligible_for_cache`

鮮度評価を使う場合は、次も重要です。

- `version_or_etag`

### Output: policy.json

`qc optimize` の出力です。

- `solver`
  - `solver_name`
  - `objective_value`
  - `solve_time_ms`
  - `shadow_price`
  - `optimality_gap`
  - `capacity_bytes`
  - `cached_bytes`
- `decisions`
  - object ごとの cache decision

### Output: metrics.json

`qc simulate` の出力です。

- replay 結果の metrics
- solver metadata 由来の診断値

## Presets

V0.1.0 では次の preset が使えます。

- `ecommerce`
  - レイテンシと鮮度を比較的重視
- `media`
  - stale 許容寄り
- `api`
  - invalidation ベースで stale を強く避ける

迷ったらまず `ecommerce` で動作確認するのが無難です。

## When to Use ILP

`Greedy` は通常運用向けです。

- 高速
- large trace でも扱いやすい

`ILP` は検証向けです。

- 小さめの問題で greedy の gap を確認したい
- score 設計を変えたときの sanity check をしたい

大きい入力に対して常用するものではありません。

## Known Caveats

- `EconomicGreedy` は静的ポリシー、`LRU` / `GDSF` は動的ポリシーです
- scoring は期待値モデル、replay は実現値評価です
- V0.1.0 は TTL 自体を最適化しません
- provider API 連携はまだありません
- QUBO の二次項は未導入です

## Recommended First Run

最初の確認はこの順序で十分です。

```bash
cargo build --workspace --release

cargo run --release -p qc-cli -- generate \
  --num-objects 1000 \
  --num-requests 100000 \
  --output trace.csv

cargo run --release -p qc-cli -- optimize \
  --input trace.csv \
  --output policy.json \
  --capacity 5000000 \
  --preset ecommerce

cargo run --release -p qc-cli -- simulate \
  --input trace.csv \
  --policy policy.json

cargo run --release -p qc-cli -- compare \
  --input trace.csv \
  --capacity 5000000 \
  --preset ecommerce
```

## Related Docs

- [README.md](/Users/shutoide/Developer/quant-cache/README.md)
- [v1-design.md](/Users/shutoide/Developer/quant-cache/docs/v1-design.md)
- [formulation.md](/Users/shutoide/Developer/quant-cache/docs/formulation.md)
- [testing-strategy.md](/Users/shutoide/Developer/quant-cache/docs/testing-strategy.md)
