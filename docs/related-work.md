# quant-cache Related Work

**Version:** 1.0
**Date:** 2026-03-31
**Status:** Confirmed

---

## 1. Research Landscape

キャッシュ置換・政策決定の研究は3つの世代に分類できる。
quant-cache は第4のアプローチとして位置づける。

```text
Generation 1: Heuristic-based (LRU, LFU, FIFO)
Generation 2: Adaptive-heuristic (ARC, LIRS, RRIP, GDSF)
Generation 3: Learning-based (DRL, Imitation Learning, MAB)
Generation 4: Optimization-based (quant-cache) ← HERE
```

---

## 2. Classical Adaptive Algorithms

### LIRS (Jiang & Zhang, 2002) [Paper 01]

Inter-Reference Recency (IRR) を用いてブロックを LIR/HIR に分類。
LRU のループ耐性の弱さを克服。最大 4× の改善。

**quant-cache との関係:** IRR は reuse distance の一形態。V1.5 で
BenefitCalculator の demand estimation に reuse distance 指標を検討する際の基礎。

### ARC (Megiddo & Modha, 2003) [Paper 11]

T1(recency) と T2(frequency) の2リストにゴーストリストを加え、
バランスパラメータ p をオンライン学習。ユーザーパラメータ不要。

**quant-cache との関係:** ARC の adaptive p は V1.5 の offline coefficient
calibration の先行事例。ただし ARC はオンライン eviction、quant-cache は
オフライン政策最適化であり、問題設定が異なる。

### RRIP (Jaleel et al., 2010) [Paper 02]

2-bit の Re-Reference Prediction Value (RRPV) でブロックの再利用距離を予測。
LRU に対して 10% 改善、ハードウェアオーバーヘッドは LRU の 1/2。

**quant-cache との関係:** RRPV は離散化された reuse distance predictor。
V2 の二次項（co-access）の重み推定に応用可能。

### AdaptiveClimb (Berend et al., 2025) [Paper 09]

LRU(full jump) と CLIMB(jump=1) を補間する adaptive promotion +
キャッシュサイズの動的調整。SIEVE に対して 10-15% 改善。

**quant-cache との関係:** 動的サイズ調整は knapsack の容量制約 C(t) の
動的変更に直接対応する。V2 の scenario optimization 候補。

---

## 3. Learning-Based Approaches

### Catcher (Zhou et al., 2022) [Paper 04]

DDPG で LRU/LFU の混合確率を学習。OPT の 2-20% 以内。
ARC に対して 32% 高い hit rate。

### MADQN (Dassanayake et al., 2024) [Paper 05]

CDN メッシュネットワークで協調型 Multi-Agent DQN。
エッジサーバ間で Q 値を重み付き更新。LRU に対して 12.5% 改善。

### PARROT (Liu et al., 2020) [Paper 07]

Belady's optimal を教師信号とした LSTM imitation learning。
LRU に対して 61% 改善 (Web Search)。Glider に対して 20% 改善。

### ML Cache Review (Krishna, 2025) [Paper 06]

Glider (+14.7% IPC), LeCaR (ARC の 18× 改善), DeepCache (LSTM),
Seq2Seq (+77% over LRU) 等を網羅的にレビュー。

**quant-cache との位置づけ:**

ML/DRL 系は最高の hit rate を達成するが、以下の制約がある:
- 学習データ・再学習が必要
- 推論のオーバーヘッド
- ブラックボックス（説明不可能）
- edge 導入が困難

quant-cache は hit rate 単体では ML に劣るが:
- 学習不要
- O(n log n) の実行コスト
- 解釈可能（ScoreBreakdown）
- 経済目的に直接最適化
- 制約を明示的に扱える
- ILP による最適性保証

→ **explicit optimization for operators, not black-box policy learning**

---

## 4. Analytical/Theoretical Foundations

### Unified Caching Analysis (Martina et al., 2016) [Paper 03]

Che's approximation を拡張し、k-LRU, q-LRU, FIFO, RANDOM を統一解析。
k-LRU が漸近最適で traffic-independent であることを証明。

**quant-cache との関係:** 解析フレームワークが提供する性能上界は、
quant-cache の optimizer が達成可能な範囲の理論的参照点になる。

### LRU with Invalidation Model (2018) [Paper 14]

Che's approximation を TTL/invalidation 下に拡張:
`phit(m) ≈ λm/(λm+μm) × [1 - e^(-(λm+μm)·TC')]`

**quant-cache との関係:** この式は quant-cache の FreshnessModel の理論的基礎。
TTL-Only モデルの P(stale) = 1 - exp(-update_rate × ttl) は
この論文の知見と整合している。

### PRP - Probabilistic Replacement (TACO) [Paper 15]

reuse distance の分布全体を推定し、最適置換下の hit probability を計算。
ML なしで ML に近い性能。

**quant-cache との関係:** V1.5 の reuse-distance-aware scoring の理論的基礎。
確率的重み付けは knapsack の benefit 計算を精緻化する。

### Reuse Distance & Stream Detection (Keramidas, 2007) [Paper 16]

PC ベースの reuse distance predictor + ストリーム検出。
LRU に対して平均 17.2% IPC 改善。

**quant-cache との関係:** ストリーム検出は knapsack の制約として
「streaming object は選択不可」を表現できる（eligible_for_cache の拡張）。

---

## 5. Surveys & Empirical Comparisons

### Web Cache Replacement Survey (Podlipnig, 2003) [Paper 13]

30+ の Web キャッシュ置換ポリシーを体系化。
GDSF の multi-factor objective (frequency × cost / size) を分析。

**quant-cache との関係:** GDSF は quant-cache の最も近い祖先。
ただし GDSF はオンライン優先度規則、quant-cache はオフライン制約付き最適化。

### Performance Comparison (Zulfa et al., 2023) [Paper 12]

実プロキシトレースで LRU, LFU, LFUDA, GDS, GDSF, SIZE, FIFO を比較。
「万能なアルゴリズムは存在しない」が主結論。

**quant-cache との関係:** 「万能でない」という知見こそが、
workload-adaptive な最適化アプローチの必要性を裏付ける。

### Distributed Caching (Mayer & Richards, 2025) [Paper 10]

分散キャッシュ（DHT, CDN, Cloud）のアーキテクチャ別に比較。
TLRU (Time-Aware LRU) の freshness 制約を分析。

**quant-cache との関係:** quant-cache V1 は集中型最適化。
分散 QUBO は V2.5 以降の課題。

---

## 6. Novelty of quant-cache

### 既存手法との差別化

| Approach | Type | Strength | Limitation |
|----------|------|----------|------------|
| GDSF | Online priority | Cost-aware, simple | Local greedy, no global optimization |
| ARC | Online adaptive | Self-tuning, O(1) | No economic objective |
| Catcher/PARROT | Offline learning | Highest hit rates | Training cost, black-box |
| **quant-cache V1** | **Offline optimization** | **Near-optimal with ILP verification, interpretable, constraint-aware** | **Static, no online adaptation** |
| **quant-cache V2** | **Quadratic optimization** | **Pairwise interactions (co-access, grouping)** | **Scalability for large n** |

### quant-cache が埋めるギャップ

文献にはこのギャップが存在する:

> **Formal constrained optimization for CDN cache policy with explicit economic
> objective, freshness-aware cost terms, and reproducible trace-based evaluation.**

1. GDSF は近いが、制約なし・局所的
2. ML/DRL は強いが、non-interpretable・高コスト
3. 理論解析は性能予測のみで、政策生成しない
4. V2 の pairwise interaction (co-access, purge-group, origin-group) は、
   我々の知る限り、既存のキャッシュ政策文献で明示的に定式化されていない

---

## 7. Key Papers for Each quant-cache Version

| Version | Most Relevant Papers | What We Take |
|---------|---------------------|-------------|
| V1 | 19(GDS/GDSF), 29(Dantzig), 13(survey), 14(invalidation) | 経済定式化 = GDSF一般化 + Dantzig knapsack |
| V1.5 | 18(Belady), 15(PRP), 16(reuse distance), 11(ARC) | Belady oracle, reuse distance, calibration |
| V2 | 23(Lucas QUBO), 24(S3-FIFO), 25(SIEVE) | quadratic formulation, modern baselines |
| V2.5 | 28(CacheLib), 22(AdaptSize) | production architecture, size-aware admission |
| V3 | 23(Lucas QUBO) | quantum backend (Ising mapping) |
| Baselines | 20(LRB), 21(TinyLFU), 26(LeCaR), 27(CACHEUS) | ML/adaptive comparisons |

---

## 8. Citation Table

| ID | Authors | Year | Title | Venue |
|----|---------|------|-------|-------|
| 01 | Jiang, Zhang | 2002 | LIRS: An Efficient Low Inter-reference Recency Set Replacement Policy | SIGMETRICS |
| 02 | Jaleel, Theobald, Steely, Emer | 2010 | High Performance Cache Replacement Using Re-Reference Interval Prediction | ISCA |
| 03 | Martina, Garetto, Leonardi | 2016 | A Unified Approach to the Performance Analysis of Caching Systems | INFOCOM |
| 04 | Zhou, Wang, Shi, Feng | 2022 | An End-to-End Automatic Cache Replacement Policy Using Deep Reinforcement Learning | — |
| 05 | Dassanayake, Wang, Hameed, Yang | 2024 | Multi-Agent Deep-Q Network-Based Cache Replacement Policy for CDN | — |
| 06 | Krishna | 2025 | Advancements in Cache Management: A Review of ML Innovations | IIT Ropar |
| 07 | Liu, Hashemi, Swersky, Ranganathan, Ahn | 2020 | An Imitation Learning Approach for Cache Replacement | ICML |
| 08 | — | 2010 | An Adaptive Dynamic Replacement Approach for Prefix Cache | — |
| 09 | Berend, Dolev, Kumari, Mishra, Kogan-Sadetsky, Somani | 2025 | DynamicAdaptiveClimb: Adaptive Cache Replacement with Dynamic Resizing | — |
| 10 | Mayer, Richards | 2025 | Comparative Analysis of Distributed Caching Algorithms | — |
| 11 | Megiddo, Modha | 2003 | ARC: A Self-Tuning, Low Overhead Replacement Cache | FAST |
| 12 | Zulfa, Fadli, Permanasari, Ahmed | 2023 | Performance Comparison of Cache Replacement Algorithms on Various Internet Traffic | — |
| 13 | Podlipnig, Böszörményi | 2003 | A Survey of Web Cache Replacement Strategies | ACM Computing Surveys |
| 14 | — | 2018 | Modeling LRU Cache with Invalidation | Computer Networks |
| 15 | — | — | Reuse Distance-Based Probabilistic Cache Replacement | ACM TACO |
| 16 | Keramidas, Petoumenos, Kaxiras | 2007 | Cache Replacement Based on Reuse Distance Prediction and Stream Detection | — |
| 17 | — | 2021 | Survey on Different Cache Replacement Algorithms (Flash/SSD) | IJEAT |
| 18 | Belady | 1966 | A Study of Replacement Algorithms for a Virtual-Storage Computer | IBM Systems Journal |
| 19 | Cao, Irani | 1997 | Cost-Aware WWW Proxy Caching Algorithms (GDS/GDSF) | USITS |
| 20 | Song, Berger, Li, Lloyd | 2020 | Learning Relaxed Belady for CDN Caching (LRB) | NSDI |
| 21 | Einziger, Friedman, Manes | 2017 | TinyLFU: A Highly Efficient Cache Admission Policy | ACM TOS |
| 22 | Berger, Sitaraman, Harchol-Balter | 2017 | AdaptSize: Orchestrating the Hot Object Memory Cache in a CDN | NSDI |
| 23 | Lucas | 2014 | Ising Formulations of Many NP Problems (QUBO cookbook) | Frontiers in Physics |
| 24 | Yang, Zhang, Qiu, Yue, Rashmi | 2023 | FIFO Queues are All You Need (S3-FIFO) | SOSP |
| 25 | Zhang, Yang, Yue, Vigfusson, Rashmi | 2024 | SIEVE is Simpler than LRU | NSDI (Best Paper) |
| 26 | Vietri et al. | 2018 | Driving Cache Replacement with ML-based LeCaR | HotStorage |
| 27 | Rodriguez et al. | 2021 | Learning Cache Replacement with CACHEUS | FAST |
| 28 | Berg, Berger et al. | 2020 | The CacheLib Caching Engine | OSDI |
| 29 | Dantzig | 1957 | Discrete-Variable Extremum Problems (Knapsack) | Operations Research |
