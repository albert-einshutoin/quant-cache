# quant-cache Value Transition Proposal

**Date:** 2026-04-02  
**Status:** Draft

## 1. Executive Summary

`quant-cache` の V1 は、economic cache decision framework として正しい。
しかし、最も大きいプロダクト価値が出る形は、単なる評価フレームワークではなく
**policy synthesis / admission compiler** である。

提案:

- 現在の `economic evaluation + replay + bounded optimality verification` を土台として維持する
- その上で、`SIEVE` / `S3-FIFO` / `TinyLFU` などの強い runtime policy を backend とし、
  `quant-cache` は **admission / prewarm / planning layer** を生成する方向へ進化させる
- 量子インスパイア最適化は **実運用時ではなく設計時のみ** 用いる
- 実運用には、探索済み policy を **軽量バイナリ** として配布する

一言で言うと:

> quant-cache should evolve from an economic evaluation framework
> into a cache policy compiler for admission and planning.

---

## 2. Current State

V1 の現在地:

- economic objective を明示した cache decision framework
- trace replay による reproducible evaluation
- `Greedy` / `ILP` による bounded optimality verification
- `LRU`, `GDSF`, `SIEVE`, `S3-FIFO` との比較基盤

V1 の価値:

- hit rate だけでは見えない cost / freshness tradeoff を可視化できる
- GDSF のような高 hit-rate policy が、economic objective では負になりうることを示せる
- cache policy を heuristic ではなく objective / constraint として議論できる

現時点の限界:

- `EconomicGreedy` 単体では `SIEVE` / `S3-FIFO` の runtime eviction 性能に勝てない
- static policy は online adaptive policy に構造的に不利
- V1 は「何が良い policy かを測る」ことはできるが、「実運用で使う最良 policy を合成して配る」ところまでは行っていない

---

## 3. Highest-Value Product Form

最もバリューが高い形は次である。

### 3.1 Product Definition

`quant-cache` を次のように定義し直す:

> traffic / cost / freshness requirements を入力すると、
> 最適な cache admission / prewarm / policy composition を
> 量子インスパイア最適化で設計し、
> 実行時は軽量な runtime policy binary として配布する framework

### 3.2 Why This Is Higher Value

この形が高価値な理由:

- 実運用時は `SIEVE` / `S3-FIFO` のような軽量 backend を使える
- 顧客には「説明可能な decision artifact」を渡せる
- 量子インスパイア最適化は offline / batch 設計問題に限定できる
- solver の重さを運用時に持ち込まない
- `hit rate champion` ではなく `business objective optimizer` として差別化できる

### 3.3 Recommended Product Positioning

採用すべき位置づけ:

- `quant-cache is not a better universal eviction policy`
- `quant-cache is an economic cache decision framework`
- `Its next high-value form is a policy synthesis and admission compiler`

---

## 4. Gap Between Current Direction and Highest-Value Form

差異は「方向性の矛盾」ではなく「重心の違い」である。

| Dimension | Current quant-cache | Highest-value form |
|-----------|---------------------|--------------------|
| 主役 | evaluation framework | policy compiler |
| 主な出力 | metrics / comparison / reference optimizer | deployable admission policy artifact |
| 中心課題 | objective design と replay | admission + eviction composition |
| solver の役割 | 経済政策の評価 | 実運用 policy の設計・探索 |
| runtime | reference optimizer replay | lightweight compiled policy |
| 量子インスパイア | roadmap 上の solver backend | policy search / synthesis engine |

要するに:

- 今の `quant-cache` は **judge**
- 目指すべき `quant-cache` は **builder**

---

## 5. Proposed Strategic Shift

### 5.1 Keep

残すべきもの:

- economic objective
- freshness-aware replay
- ILP / exact verification on bounded instances
- baseline comparison framework
- synthetic + real trace evaluation

これらは将来の compiler の信頼性を支える基盤である。

### 5.2 Change

変えるべき重心:

- `EconomicGreedy` を主役から **reference optimizer** へ下げる
- `SIEVE` / `S3-FIFO` / `TinyLFU` を runtime backend 候補として扱う
- `quant-cache` の本体を **admission / prewarm / planning decision layer** に寄せる
- runtime ではなく **design-time optimization** を前面化する

### 5.3 New Core Question

今後の中心問いはこれに変える:

> Which runtime cache architecture and admission logic should be deployed
> for this workload, given business cost, freshness constraints, and capacity?

これは V1 の
`which objects should be cached in a static set?`
より、プロダクト価値が大きい。

---

## 6. Product Architecture Proposal

### 6.1 Architecture Layers

```text
Traffic / Cost / Freshness Data
            │
            ▼
   quant-cache Design Engine
   - replay evaluation
   - economic objective
   - policy search
   - hybrid composition
   - quantum-inspired optimization
            │
            ▼
  Compiled Policy Artifact
  - admission thresholds
  - object class rules
  - prewarm targets
  - backend selection
            │
            ▼
  Runtime Execution Layer
  - SIEVE / S3-FIFO / TinyLFU backend
  - bit-optimized binary
```

### 6.2 Runtime Policy Candidates

runtime backend 候補:

- `SIEVE`
- `S3-FIFO`
- `TinyLFU` / `W-TinyLFU`
- `AdaptSize`

`quant-cache` は、これらを置き換えるのではなく、
**組み合わせ・選定・補正する layer** になる。

### 6.3 Compiler Output

将来の配布成果物:

- backend choice
- admission threshold
- size-based filtering rule
- freshness-based exclusion rule
- prewarm candidate list
- optional segment / queue configuration

出力形式の例:

- Rust/C ABI library
- WASM policy module
- generated config + binary decision tables

---

## 7. Quantum-Inspired Role

### 7.1 What Quantum-Inspired Should Optimize

量子インスパイア最適化の役割は runtime ではなく、次のような offline 問題に限定する。

- admission threshold search
- multi-policy composition search
- prewarm set optimization
- purge-group consistency optimization
- origin-group shielding
- quadratic interaction refinement

### 7.2 What Quantum-Inspired Should Not Do

避けるべき方向:

- per-request runtime eviction decision
- direct replacement of `SIEVE` / `S3-FIFO`
- low-latency inner-loop execution

### 7.3 Why This Matches the Product

この設計なら:

- 量子インスパイアの計算コストを offline に閉じ込められる
- runtime は deterministic, auditable, fast
- enterprise 導入が容易になる

---

## 8. New Algorithm Opportunities

最も有望なのは「単独新 eviction」ではなく「hybrid decision algorithm」である。

候補:

### A. EconomicSieveGate

- runtime backend: `SIEVE`
- admission gate: economic score threshold
- 利点: 実装容易、SIEVE の強みを維持
- 課題: threshold tuning

### B. EconomicS3FifoPlanner

- runtime backend: `S3-FIFO`
- planner: prewarm / protected-segment bias
- 利点: scan-heavy / one-hit-wonder に強い
- 課題: segment-level control の設計

### C. Hybrid Policy Search

- 候補 policy を DSL 的に組み合わせる
- 量子インスパイア探索で構造選択
- 利点: 最も差別化しやすい
- 課題: 探索空間設計が難しい

推奨順位:

1. `EconomicSieveGate`
2. `EconomicS3FifoPlanner`
3. `Hybrid Policy Search`

---

## 9. Roadmap Proposal

### Phase A — Repositioning (near-term)

目標:

- V1 の位置づけを evaluation framework として確定
- README / docs / examples を compiler vision に寄せる

成果物:

- messaging refresh
- architecture diagram refresh
- `reference optimizer` 明記

### Phase B — Admission Compiler (highest priority)

目標:

- `SIEVE` / `S3-FIFO` backend に対して admission gate を生成する

成果物:

- policy DSL
- threshold search
- generated runtime config
- multi-trace objective evaluation

### Phase C — Policy Synthesis Engine

目標:

- backend selection + admission rule + prewarm set を自動探索

成果物:

- policy search space definition
- candidate architecture evaluator
- compiled policy artifact

### Phase D — Quadratic / Quantum-Inspired Search

目標:

- pairwise / group interactions を使った上位設計探索

成果物:

- quadratic policy search objective
- classical SA backend
- quantum-inspired backend
- small-scale research comparison

---

## 10. Business Value

この方向での価値提案:

### For CDN / platform teams

- cache tuning を属人的ルールから objective-driven design に変えられる
- コスト、鮮度、容量を同時に扱える
- 実行時は軽いので production 導入しやすい

### For infra SaaS

- customer-specific cache policy を自動生成できる
- backend policy を変えずに上位設計レイヤだけ差別化できる

### For research / credibility

- objective design
- replay evaluation
- bounded optimality
- hybrid policy synthesis
- quantum-inspired search

が一つの連続した研究ラインになる

---

## 11. Risks

| Risk | Description | Mitigation |
|------|-------------|------------|
| 探索空間が広すぎる | hybrid synthesis が複雑になりすぎる | まず threshold / backend choice に限定 |
| admission gate が純 SIEVE に勝てない | workload により逆効果 | backend-specific gate design と train/validation 分離 |
| 量子色が先行しすぎる | gimmick に見える | runtime 価値を前面、quantum は設計時 backend に留める |
| artifact 配布が複雑 | 顧客環境統合が難しい | generated config → static lib → plugin の順に段階導入 |

---

## 12. Recommendation (Revised after Codex review)

**Codex review により framing を修正:**

- ❌ "policy compiler" → ✅ **"economic cache control plane"**
- ❌ 独自 runtime binary → ✅ **vendor-native config/code generation**
- ❌ EconomicSieveGate が最初 → ✅ **Policy IR + evaluator が最初**
- ❌ QUBO で knapsack → ✅ **QUBO で policy DSL 空間を探索**

推奨方針:

1. `quant-cache` の V1 evaluation framework はそのまま維持する
2. 主戦場を **economic cache control plane** に移す
3. 最初の施策として **Policy IR** (backend + admission + bypass + prewarm) を定義する
4. IR を `qc-simulate` で replay 評価できるようにする
5. 1つの vendor (Cloudflare) 向けに config generation を作る
6. 量子インスパイア最適化は **policy DSL 空間の探索 backend** として使う
7. Runtime は **vendor-native config / edge code** で配布する

最終的な定義:

> quant-cache is an economic cache control plane that evaluates cache policies
> through explicit economic objectives, searches the policy design space using
> quantum-inspired optimization, and generates vendor-native cache configurations.

詳細: [2026-04-02_strategic-direction.md](2026-04-02_strategic-direction.md)
