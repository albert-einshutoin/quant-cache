# Strategic Direction — Economic Cache Control Plane

**Date:** 2026-04-02
**Participants:** Claude (Opus 4.6) + Codex (GPT)
**Input:** docs/value-transition-proposal.md

---

## 1. Codex の判定

### value-transition-proposal.md の評価

> 「policy compiler」は半分正しく、半分ズレている。
> 正しいのは「実運用に効く artifact を出すべき」という方向。
> ズレているのは、市場が欲しいのは "cache policy compiler" ではなく
> 「既存 CDN 設定を経済目的で最適化して、そのまま適用できる推奨/生成物」。

### 修正された Product Framing

| Proposal | Codex 修正 |
|----------|-----------|
| "policy compiler" | **"economic cache control plane"** |
| 独自 runtime binary を配布 | **vendor-native config/code を生成** |
| EconomicSieveGate が最初 | **Policy IR + evaluator が最初** |
| QUBO/SA で knapsack を解く | **QUBO/SA で policy DSL 空間を探索** |

---

## 2. 市場分析

### 既存 CDN の実行面

各 CDN は独自の cache control 機構を持っている:
- Cloudflare: Cache Rules + Workers
- Fastly: VCL + Compute
- CloudFront: Functions + Lambda@Edge
- Akamai: Property Manager + EdgeWorkers

→ **新しい独自 runtime は不要。既存の rules engine 向けに生成するのが正解。**

### 需要のある場所

| 需要 | 強さ | quant-cache の関係 |
|------|------|-------------------|
| CDN-native cache controls | 強い | 出力先 |
| Analytics / cost visibility | 強い | 評価基盤 |
| Edge customization | 強い | artifact 配布先 |
| Cross-vendor objective-aware 設定生成 | **ギャップあり** | **ここが勝負どころ** |
| 独自 cache runtime | 弱い | やらない |

### Competitive Gap

> "cache policy as a service" の明確な市場カテゴリは薄い。
> ただし cross-CDN で objective-aware に設定生成する層は弱い。
> ここなら勝負できる。

---

## 3. Quantum-Inspired の正しい使い方

### 効かない場所
- 単一 knapsack (greedy gap 0.01% で十分)
- per-request runtime eviction
- SIEVE/S3-FIFO の代替

### 効く場所
- **backend 選択 + admission + TTL + prewarm の同時探索** (離散構造最適化)
- Purge-group / origin-group / co-access の相互作用付き最適化
- 複数セグメント・複数制約の構成探索
- Cross-CDN / multi-tier cache planning

→ **QUBO/SA は "solver for policy search over a richer DSL" なら意味がある。**

---

## 4. Compiled Artifact の需要順

| 形式 | 需要 | 理由 |
|------|------|------|
| **Generated config + decision tables** | **最高** | CDN の rules/config を直接触れる |
| **Vendor-native edge code** | 高い | Workers/VCL に落とせる |
| WASM module | 中 | Fastly Compute / Cloudflare Workers 文脈 |
| Rust/C ABI library | 低い | CDN オペレータが組み込む前提が狭い |

---

## 5. 確定方針

### Product Definition (最終)

> quant-cache は **economic cache control plane** である。
> traffic / cost / freshness data を入力し、
> 量子インスパイア最適化で policy DSL 空間を探索し、
> vendor-native config / edge code として配布する。

### 3つの柱

1. **Economic Evaluation** — hit rate だけでは見えないコストを可視化 (V1, done)
2. **Policy Search** — backend/admission/bypass/prewarm 空間を探索 (partial)
3. **Deployment Scaffold** — Cloudflare deployment scaffold を生成 (partial)

### Quantum-Inspired の位置づけ

- **Design-time only** — offline batch で policy search に使用
- **Runtime は deterministic** — 探索結果を compiled config/binary に落とす
- **QUBO/SA は主役ではなく backend** — policy DSL 探索の solver

---

## 6. 次の3ステップ (確定)

### Step 1: Policy IR を作る

中間表現:
```rust
struct PolicyIR {
    backend: Backend,              // SIEVE | S3FIFO | TinyLFU
    admission_rule: AdmissionRule, // always | score > τ | score/size > τ
    bypass_rule: BypassRule,       // freshness_risk > τ | size > τ
    prewarm_set: Vec<String>,      // top-k by objective
    ttl_class_rules: Vec<TtlClassRule>,
    cache_key_rules: Vec<CacheKeyRule>,
}
```

quant-cache の出力を JSON policy file から Policy IR に上げる。

### Step 2: IR Evaluator を作る

`qc-simulate` で IR を replay できるようにする。
`SIEVE + threshold`, `S3FIFO + size cap`, `prewarm + backend` を
同じ枠で比較可能にする。

### Step 3: 1つの vendor 向け compiler を作る

最初のターゲット: **Cloudflare Cache Rules** (最も API が整っている)
IR → Cloudflare Cache Rules JSON/Workers script を生成。

---

## 7. やらないこと (明示)

- 独自 cache runtime の配布
- per-request runtime での QUBO/SA 実行
- SIEVE/S3-FIFO の代替を主張
- 「量子」を marketing の前面に出す

---

## 8. Roadmap (修正版)

| Phase | Focus | Deliverable |
|-------|-------|-------------|
| **A** (current) | Economic evaluation framework | Done (V1-V2) |
| **B** (next) | Policy IR + IR evaluator | DSL 型定義 + replay |
| **C** | Policy search engine | SA/QUBO で DSL 空間探索 |
| **D** | Vendor-native compiler | Cloudflare Cache Rules 生成 |
| **E** | Multi-vendor + quantum | Cross-CDN + quantum backend |
