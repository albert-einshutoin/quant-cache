# quant-cache Mathematical Formulation

**Version:** 1.2
**Date:** 2026-04-01
**Status:** Implemented and verified

---

## 1. Problem Statement

限られたキャッシュ容量の中で、経済的価値が最大となるオブジェクト集合を選択する。

---

## 2. V1: Linear Knapsack Formulation

### 2.1 Decision Variable

```
x_i ∈ {0, 1}    for i = 1, ..., n

x_i = 1: オブジェクト i をキャッシュする
x_i = 0: キャッシュしない
```

### 2.2 Objective Function

時間窓 T における期待純便益の最大化:

```
maximize  Σ_i  b_i * x_i
```

ここで `b_i` は以下で定義される期待純便益:

```
b_i = benefit_i - freshness_cost_i

benefit_i = E[req_i(T)] * (
              latency_saving_i * λ_latency
            + origin_cost_i
            )
```

`freshness_cost_i` は Freshness Model によって異なる（後述 §4）。

### 2.3 Parameters

| Symbol | 定義 | 単位 |
|--------|------|------|
| `E[req_i(T)]` | 時間窓 T 内のオブジェクト i への期待リクエスト数 | requests |
| `latency_saving_i` | cache hit 時のレイテンシ削減量 | ms |
| `λ_latency` | レイテンシの経済価値 | $/ms |
| `origin_cost_i` | origin fetch のコスト（帯域・compute） | $/request |

### 2.4 Constraint

容量制約:

```
Σ_i  s_i * x_i  ≤  C

s_i: オブジェクト i のサイズ (bytes)
C:   キャッシュ容量 (bytes)
```

### 2.5 Complete V1 Formulation

```
maximize    Σ_i  b_i * x_i

subject to  Σ_i  s_i * x_i  ≤  C
            x_i ∈ {0, 1}      ∀i
```

これは **0-1 ナップサック問題** である。
greedy（benefit/size ratio ソート）で近似解が得られるが、最適解は保証されない。
ILP で厳密解が得られ、V1 では greedy の解品質評価に使用する。

---

## 3. Greedy Solver

### 3.1 Algorithm

```
1. 各オブジェクト i について b_i を計算
2. efficiency_i = b_i / s_i を計算
3. efficiency_i の降順にソート
4. 容量制約を満たす限り、順に x_i = 1 とする
```

計算量: O(n log n)

### 3.2 Variant: Pure Benefit

大きなオブジェクトがサイズペナルティで不利になりすぎる場合の補助:

```
1. b_i の降順にソート
2. 容量制約を満たす限り、順に x_i = 1 とする
```

V1 では両方を実行し、目的関数値が高い方を採用する。

---

## 4. Freshness Model

stale penalty と invalidation cost を同時に計上すると二重計上になるため、
V1 ではどちらか一方のモデルを選択する。

### 4.1 TTL-Only Model

invalidation を行わない運用を仮定。TTL 超過後に stale レスポンスを返すリスクを計上。

更新がポアソン過程に従うと仮定:

```
P(stale_i) = 1 - exp(-update_rate_i * ttl_i)

E[stale_events_i(T)] = E[req_i(T)] * P(stale_i)

freshness_cost_i = E[stale_events_i(T)] * stale_penalty_i
```

| Symbol | 定義 | 単位 |
|--------|------|------|
| `update_rate_i` | オブジェクト i の更新頻度 | updates/sec |
| `ttl_i` | 設定された TTL（V1 では入力固定値、最適化対象外） | sec |
| `stale_penalty_i` | stale レスポンス 1 回あたりのペナルティ | $/event |

### 4.2 Invalidation-on-Update Model

更新のたびに即座に invalidation/purge を行う運用を仮定。stale は発生しない。

```
E[invalidations_i(T)] = update_rate_i * T

freshness_cost_i = E[invalidations_i(T)] * invalidation_cost
```

| Symbol | 定義 | 単位 |
|--------|------|------|
| `invalidation_cost` | 1 回の invalidation/purge コスト | $/event |

### 4.3 Model Selection

V1 ではシナリオ設定で切り替える。同一トレースに対して両モデルで評価し、
結果の乖離を観察することで、モデル選択の感度を把握する。

---

## 5. Capacity Shadow Price (Lagrangian)

容量制約のラグランジュ乗数 μ は、「1バイト追加のキャッシュ空間の限界価値」を表す。

```
L(x, μ) = Σ_i b_i x_i - μ (Σ_i s_i x_i - C)
         = Σ_i (b_i - μ s_i) x_i + μC
```

最適な μ* のもとで:

```
x_i* = 1  if  b_i / s_i > μ*
x_i* = 0  if  b_i / s_i < μ*
```

greedy solver の efficiency threshold は μ* の近似値に対応する。
V1 では μ* を greedy のカットオフ点として報告し、容量の限界価値の指標とする。

---

## 6. V2 Extension: Quadratic Terms (Preview)

V2 では以下の二次相互作用を導入予定:

### 6.1 Co-Access Bonus

同時にアクセスされやすいオブジェクトペアに対するボーナス:

```
+ Σ_{i<j}  coAccess_{ij} * x_i * x_j
```

### 6.2 Origin Grouping Bonus

同一オリジンのオブジェクトをまとめてキャッシュすることで
origin 負荷を大幅に削減できる場合のボーナス:

```
+ Σ_{i<j, same_origin}  originBonus_{ij} * x_i * x_j
```

### 6.3 Purge Batch Penalty

同一パージグループのオブジェクトが部分的にキャッシュされると
パージ操作が複雑になるペナルティ:

```
- Σ_{i<j, same_purge_group}  purgePenalty_{ij} * x_i * (1-x_j)
```

### 6.4 QUBO Form

V2 の目的関数は:

```
maximize  Σ_i h_i x_i + Σ_{i<j} J_{ij} x_i x_j

subject to capacity constraint (penalty term or explicit)
```

これが標準的な **QUBO** (Quadratic Unconstrained Binary Optimization) であり、
simulated annealing、量子アニーリング、その他のメタヒューリスティクスで解ける。

---

## 7. Unit Consistency Check

全項が `$/period` に統一されていることの確認:

| Term | Calculation | Unit |
|------|-------------|------|
| Latency benefit | requests × ms × $/ms | $ |
| Origin cost saving | requests × $/request | $ |
| Freshness cost (TTL-Only) | requests × P(stale) × $/event | $ |
| Freshness cost (Invalidation) | updates/sec × sec × $/event | $ |
| **Net benefit** | **$ - $** | **$** |

注: freshness cost は選択したモデルに応じて一方のみ計上される。

---

## 8. Sensitivity Parameters

V1 で係数感度分析を行うべきパラメータ:

| Parameter | Range | Impact |
|-----------|-------|--------|
| `λ_latency` | 0.0001 - 0.01 $/ms | レイテンシ vs コストの重み |
| `stale_penalty` | 0.001 - 1.0 $/event | 鮮度の重視度 |
| `invalidation_cost` | 0.0001 - 0.1 $/event | purge コスト感度 |
| `capacity` | 10% - 90% of total size | 容量制約の厳しさ |
| Zipf α | 0.5 - 1.5 | 人気分布の偏り |
