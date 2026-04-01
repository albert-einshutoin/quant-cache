# quant-cache V1 Design Document

**Version:** 2.0
**Date:** 2026-04-01
**Status:** Implemented — all acceptance criteria passed

---

## 1. Overview

quant-cache は CDN キャッシュ戦略を経済価値ベースで最適化するエンジンである。

**V1 定義:** Economic cache optimizer with knapsack-based policy selection

V1 では線形目的関数 + 容量制約による 0-1 ナップサック問題として定式化し、
trace replay による評価基盤を構築する。QUBO（二次項）の導入は V2 以降。

---

## 2. Goals

- 経済価値ベースの目的関数を定義し、全項を期間当たりの期待コスト/便益に統一する
- trace replay simulator を構築し、既存ベースライン（LRU, GDSF）との定量比較を可能にする
- Solver trait を定義し、greedy solver で V1 の有効性を検証する
- 小規模問題で exact ILP と比較し、greedy の解品質を評価する

## 3. Non-Goals

- QUBO 定式化（V2）
- TTL 最適化、purge/prefetch の自動実行（V1.5+）
- TTL の最適値決定（V1 では TTL は入力として与えられる固定値であり、最適化対象ではない）
- リアルタイム最適化（V3）
- CDN provider への実際のAPI呼び出し（V2+）
- マルチリージョン対応（V2+）
- 量子バックエンド接続（V3）

---

## 4. Architecture

```text
quant-cache/
├── crates/
│   ├── qc-model/        # データ型定義
│   │   ├── object.rs         # ObjectFeatures
│   │   ├── trace.rs          # RequestTraceEvent
│   │   ├── policy.rs         # PolicyDecision, CachePolicy
│   │   ├── scenario.rs       # ScenarioConfig
│   │   └── metrics.rs        # MetricsSummary, ScoreBreakdown
│   │
│   ├── qc-solver/       # Scoring + 最適化エンジン
│   │   ├── score.rs          # BenefitCalculator: ObjectFeatures + Config → ScoredObject
│   │   ├── trait.rs          # Solver trait: ScoredObject + Constraint → Result
│   │   ├── greedy.rs         # GreedySolver (ratio + pure benefit, 2系統比較)
│   │   └── ilp.rs            # ExactILP (HiGHS, 小規模検証用)
│   │
│   ├── qc-simulate/     # 評価基盤
│   │   ├── engine.rs         # TraceReplayEngine
│   │   ├── baselines.rs      # LRU, GDSF 実装
│   │   ├── comparator.rs     # ポリシー比較・レポート
│   │   └── synthetic.rs      # Synthetic trace generator
│   │
│   └── qc-cli/          # CLI インターフェース
│       ├── main.rs
│       ├── commands/
│       │   ├── optimize.rs   # solve + output policy
│       │   ├── simulate.rs   # trace replay
│       │   ├── compare.rs    # baseline comparison
│       │   └── generate.rs   # synthetic trace generation
│       └── io.rs             # CSV/Parquet I/O
│
├── data/
│   ├── samples/              # サンプルトレース
│   └── schemas/              # trace/object schema定義
│
├── analysis/                 # Python notebooks
│   ├── coefficient_tuning.ipynb
│   ├── solver_comparison.ipynb
│   └── requirements.txt
│
├── docs/
│   ├── v1-design.md          # 本ドキュメント
│   └── formulation.md        # 数学的定式化
│
└── Cargo.toml                # workspace
```

---

## 5. Data Model

### 5.1 Trace Schema Version

全 trace データは `schema_version` フィールドを持つ。V1 では `"1.0"` 固定。

### 5.2 Request Trace Event

```rust
struct RequestTraceEvent {
    schema_version: String,             // "1.0"
    timestamp: DateTime<Utc>,
    object_id: String,
    cache_key: String,                  // query param / variant を含む完全キー
    object_size_bytes: u64,
    response_bytes: Option<u64>,        // 実際の転送量 (range request 対応)
    cache_status: Option<CacheStatus>,  // Hit, Miss, Expired, Bypass
    status_code: Option<u16>,           // HTTP status (cacheable 判定用)
    origin_fetch_cost: Option<f64>,     // $/request
    response_latency_ms: Option<f64>,
    region: Option<String>,             // V1 は単一リージョン、optional
    content_type: Option<String>,
    version_or_etag: Option<String>,
    eligible_for_cache: bool,           // 動的APIレスポンス等の除外用
}
```

### 5.3 Object Features (集約後)

```rust
struct ObjectFeatures {
    object_id: String,
    cache_key: String,
    size_bytes: u64,
    eligible_for_cache: bool,
    // 時間窓 T 内の集約値
    request_count: u64,
    request_rate: f64,          // requests/sec
    avg_response_bytes: u64,    // 平均転送量 (Byte Hit Ratio 計算用)
    avg_origin_cost: f64,       // $/request
    avg_latency_saving_ms: f64, // cache hit vs miss の差
    // TTL は入力固定値 (V1 では最適化対象外)
    ttl_seconds: u64,
    update_rate: f64,           // updates/sec (staleness risk)
    last_modified: Option<DateTime<Utc>>,  // update_rate 推定根拠
    stale_penalty_class: StalePenaltyClass,
    purge_group: Option<String>,
    origin_group: Option<String>,
}
```

### 5.4 Scored Object (Solver 入力)

```rust
/// BenefitCalculator が ObjectFeatures + ScenarioConfig から生成する。
/// Solver はこの型だけを受け取り、scoring ロジックに依存しない。
struct ScoredObject {
    object_id: String,
    size_bytes: u64,
    net_benefit: f64,
    score_breakdown: ScoreBreakdown,
}
```

### 5.5 Policy Decision

```rust
struct PolicyDecision {
    cache_key: String,          // object_id ではなく cache_key で識別
    cache: bool,
    score: f64,
    score_breakdown: ScoreBreakdown,
}

struct ScoreBreakdown {
    expected_hit_benefit: f64,
    freshness_cost: f64,        // FreshnessModel に応じた値
    net_benefit: f64,
    // 診断値 (最適化項ではない)
    capacity_shadow_cost: Option<f64>,  // greedy カットオフ時の μ* × size
}
```

### 5.6 Scenario Config

```rust
struct ScenarioConfig {
    capacity_bytes: u64,
    time_window_seconds: u64,
    // 経済パラメータ
    latency_value_per_ms: f64,      // λ_latency: $/ms
    // Freshness モデル (V1 では排他選択)
    freshness_model: FreshnessModel,
}

/// stale penalty と invalidation cost の二重計上を防ぐため、
/// V1 ではどちらか一方のモデルを選択する。
enum FreshnessModel {
    /// TTL-only: invalidation しない。stale penalty のみ計上。
    TtlOnly {
        stale_penalty: StalePenaltyConfig,
    },
    /// Invalidation-on-update: 更新ごとに invalidation。stale ≈ 0。
    InvalidationOnUpdate {
        invalidation_cost: f64,  // $/invalidation
    },
}
```

Solver 選択は `ScenarioConfig` に含めず、CLI / orchestration 層で行う。

---

## 6. Solver Interface

### 6.1 Scoring と Solving の分離

```text
ObjectFeatures + ScenarioConfig
        │
        ▼
  BenefitCalculator  (qc-solver/score.rs)
        │
        ▼
  Vec<ScoredObject>
        │
        ▼
  Solver trait       (qc-solver/trait.rs)
        │
        ▼
  SolverResult
```

Solver は scoring ロジックを知らない。`ScoredObject`（benefit + size）と
`CapacityConstraint` だけを受け取る。

### 6.2 Trait 定義

```rust
struct CapacityConstraint {
    capacity_bytes: u64,
}

trait Solver {
    fn solve(
        &self,
        objects: &[ScoredObject],
        constraint: &CapacityConstraint,
    ) -> SolverResult;
}

struct SolverResult {
    decisions: Vec<PolicyDecision>,
    objective_value: f64,
    solve_time_ms: u64,
    feasible: bool,
    gap: Option<f64>,  // ILP only: optimality gap
    shadow_price: Option<f64>,  // capacity の限界価値 μ*
}
```

### 6.3 V1 Solver 実装

| Solver | 用途 | 計算量 | 備考 |
|--------|------|--------|------|
| GreedySolver | 主方式 | O(n log n) | ratio + pure benefit の2系統を実行し、目的関数値が高い方を採用 |
| ExactIlpSolver | 小規模検証 | Exact | HiGHS backend, 実装上限 n < 10,000、受け入れ試験は n <= 1,000 |

**注意:** 0-1 ナップサック問題に対する ratio greedy は最適解を保証しない。
V1 では exact ILP との比較により解品質を評価する。

---

## 7. Trace Replay Simulator

### 7.1 目的

- オフラインでポリシーを適用し、各メトリクスを計測
- 異なるポリシー間の定量比較

### 7.2 実行フロー

```text
Trace (時系列) → Simulator → PolicyDecision適用 → Metrics集計

各リクエストに対して:
  1. cache_key がキャッシュ対象か判定 (policy or baseline)
  2. Hit/Miss 判定
  3. コスト・レイテンシ計算
  4. Staleness チェック (TTL超過 or version変更)
  5. メトリクス更新
```

### 7.3 Baselines

| Baseline | アルゴリズム | 実装場所 |
|----------|------------|---------|
| LRU | Least Recently Used, 容量超過時に最古を退去 | `qc-simulate/baselines.rs` |
| GDSF | GreedyDual-Size-Frequency, コスト・サイズ・頻度考慮 | `qc-simulate/baselines.rs` |
| EconomicGreedy | V1 solver (benefit/size ratio) | `qc-solver/greedy.rs` |
| ExactILP | 整数線形計画 (小規模) | `qc-solver/ilp.rs` |

---

## 8. Metrics

### 8.1 Primary Metrics

| Metric | 定義 | 単位 |
|--------|------|------|
| Hit Ratio | cache hits / total requests | % |
| Byte Hit Ratio | bytes served from cache / total bytes | % |
| Origin Egress | origin からの転送量 | bytes |
| Estimated Cost Savings | Σ (miss_cost_baseline - miss_cost_optimized) | $ |
| Policy Objective Value | Σ expected_benefit_i * x_i | $ |

### 8.2 Diagnostic Metrics

| Metric | 定義 | 単位 |
|--------|------|------|
| Stale Serve Rate | stale responses / total | % |
| Policy Churn | 前回との差分オブジェクト数 / total | % |
| Solve Time | solver 実行時間 | ms |
| Capacity Utilization | 使用バイト / 容量制約 | % |
| Optimality Gap | (ILP best - greedy) / ILP best | % |

---

## 9. Synthetic Trace Generator

制御可能なパラメータ:

| Parameter | 説明 | デフォルト |
|-----------|------|-----------|
| num_objects | ユニークオブジェクト数 | 10,000 |
| num_requests | 総リクエスト数 | 1,000,000 |
| popularity_distribution | Zipf(α), Uniform, etc. | Zipf(0.8) |
| size_distribution | LogNormal(μ, σ), Uniform | LogNormal(10, 2) |
| update_rate_distribution | Poisson(λ) | Poisson(0.01) |
| burst_probability | バーストイベント発生確率 | 0.05 |
| time_window_seconds | シミュレーション時間窓 | 86400 (1日) |

---

## 10. V1 Acceptance Criteria

実行条件: single thread, release build, Apple Silicon (M-series)

### 10.1 性能基準

| Criteria | Target | Actual | 条件 |
|----------|--------|--------|------|
| GreedySolver | <= 1s | **< 1ms** | 10,000 pre-scored objects |
| TraceReplayEngine | <= 10s | **0.5s** | 1,000,000 events |
| CLI e2e | <= 15s | **~5s** | generate → optimize → simulate → compare |

### 10.2 品質基準

| Criteria | Target | Actual | 条件 |
|----------|--------|--------|------|
| Optimality Gap (median) | < 5% | **0.024%** | n <= 200, synthetic 50ケース |
| Optimality Gap (p95) | < 10% | **0.57%** | n <= 200, synthetic 50ケース |
| Unit consistency | Pass | **Pass** | 全 benefit/cost 項が $/period |

### 10.3 機能基準

- [x] `qc generate`: synthetic trace を生成できる
- [x] `qc optimize`: trace → ScoredObject → PolicyDecision を出力できる（`--config` TOML対応）
- [x] `qc simulate`: trace replay で metrics を計測できる
- [x] `qc compare`: LRU, GDSF, EconomicGreedy（+ optional ILP）の比較レポートを出力できる

### 10.4 ベースライン比較

- [x] LRU, GDSF に対して **定量比較が可能** であること
- [x] EconomicGreedy が LRU に対して cost savings で 16/20 ケースで優位

**Static vs Online の構造的差異について:**
EconomicGreedy はオフラインで固定するキャッシュセットを選択する静的ポリシーである。
GDSF はオンラインでアクセスパターンに適応する動的ポリシーである。
したがって、GDSF が cost savings で EconomicGreedy を上回ることは構造的に自然であり、
「GDSF を常に上回ること」は V1 の目標ではない。
V1 の価値は、GDSF にはない以下の特性にある:
- 明示的な経済目的関数に対する最適化 (ILP で検証可能)
- freshness cost の定量的な内部化
- 再現可能な trace-based 評価フレームワーク

---

## 11. Risk & Mitigation

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| Scoring 誤差が solver 差を上回る | 最適化の意味がなくなる | High | 係数感度分析、ablation study、coefficient tuning notebook |
| stale/invalidation モデルが粗く誤誘導する | 不適切な policy を推奨 | Medium | TTL-only と InvalidationOnUpdate を別シナリオで評価・比較 |
| Greedy が特定分布で極端に悪化する | 解品質の信頼性低下 | Medium | Exact ILP との比較を bounded instances で実施 |
| Synthetic trace に過適合する | 実トレースで性能未達 | Medium | CloudFront/Cloudflare 互換 schema、複数分布での評価 |
| Baseline 実装の公平性不足 | 比較結果の信頼性低下 | Low | 共通 simulator 上で比較、同一容量・同一 trace 条件を固定 |
| 大規模問題 (n > 100k) で性能劣化 | V2 移行時のボトルネック | Low | V1 は n <= 10k で検証、候補集合の事前絞り込み設計を検討 |
| LRU promote が O(n) | 大規模キャッシュで replay が遅い | Low | V2 で linked-hash-map 等に置き換え |

### Known Caveats

**Scoring vs Replay のモデル差:**
Scoring は確率的期待値モデル（Poisson P(stale) = 1 - exp(-λt)）で freshness cost を推定する。
Replay は実際の TTL 超過・version mismatch を観測して stale を判定する。
この2つは本質的に異なるモデルであり、完全には一致しない。
これは意図的な設計: 一致させすぎると自己都合評価になるため、
モデル推定 (scoring) と実現値評価 (replay) を独立に持つ方が健全である。

---

## 12. Roadmap Beyond V1

| Version | Focus | Key Addition |
|---------|-------|-------------|
| V1 | 定式化検証 | Economic knapsack + trace replay |
| V1.5 | TTL/Purge | TTL最適化、purge候補生成 |
| V2 | QUBO導入 | 二次項（co-access, origin grouping, purge batch） |
| V2.5 | Provider統合 | CloudFront/Cloudflare API接続 |
| V3 | 量子実験 | IBM Quantum / Amplify 接続 |
