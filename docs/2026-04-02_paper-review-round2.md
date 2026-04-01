# Paper Review Round 2 — QC01-QC12 Integration

**Date:** 2026-04-02
**Participants:** Claude (Opus 4.6) + Codex (GPT)

---

## 新規論文 12本の分類

### 理論的基盤 (方針を確認)
| Paper | Key Insight | quant-cache への影響 |
|-------|-------------|---------------------|
| QC12 Dantzig (1957) | GreedySolver = Dantzig's greedy knapsack | 理論的根拠を確認。50%保証、実測0.01% gap |
| QC06 Lucas (2014) | QUBO encoding cookbook | V2のQuadraticProblem設計を確認。penalty B>>A の指針 |
| QC01 Belady (1966) | MIN algorithm。LRU achieves 70-90% of MIN | BeladyPolicy が正しく実装されていることを確認 |

### 直接的な競合/ベースライン (追加すべき)
| Paper | Algorithm | なぜ重要か |
|-------|-----------|-----------|
| QC02 GDS/GDSF (1997) | GDSF: H + freq*cost/size | 我々の直接の祖先。既にベースライン化済み |
| QC07 S3-FIFO (2023) | 3-queue FIFO, quick demotion | **2023年の production standard。未実装** |
| QC08 SIEVE (2024) | Lazy promotion + quick demotion | **NSDI Best Paper。最新SOTA。未実装** |
| QC04 TinyLFU (2017) | Bloom filter admission | **Caffeine/Spring で広く使用。未実装** |
| QC05 AdaptSize (2017) | Size-aware admission, Markov model | **Akamai production。未実装** |

### ML系 (位置づけ確認)
| Paper | Algorithm | Status |
|-------|-----------|--------|
| QC03 LRB (2020) | LightGBM で Belady 近似 | CDNのML SOTA。実装コスト高、後回し |
| QC09 LeCaR (2018) | Expert mixing (regret) | CACHEUS の前身 |
| QC10 CACHEUS (2021) | Adaptive LeCaR | domain knowledge の重要性を示す |

### Production Context
| Paper | System | Key Finding |
|-------|--------|-------------|
| QC11 CacheLib (2020) | Meta production | **real Zipf α = 0.3-0.7**。我々の α=0.8 は楽観的 |

---

## Codex の判定

### 研究の中核は変えなくていい

> 「経済目的を明示した制約付き最適化」という軸は有効。

### 評価設計と位置づけは今すぐ変えるべき

| Issue | Severity | Action |
|-------|----------|--------|
| S3-FIFO/SIEVE が比較にない | **Critical** | 最優先で追加 |
| Zipf α=0.8 は楽観的 | **High** | デフォルトを 0.6 に変更、α sweep を追加 |
| SA を "QUBO" と呼ぶのは強すぎる | **High** | 名称を修正 |
| TinyLFU ベースラインなし | **Medium** | SIEVE/S3-FIFO の後に追加 |

### 主張の sharpen

**良い主張:**
> "経済目的・鮮度コスト・明示制約を扱う cache admission optimizer"

**避けるべき主張:**
- ❌ "SOTA replacement policy を超える"
- ❌ "QUBO が中核優位"
- ❌ "ML より強い"

**勝ち筋:**
> "Operators care about dollars, freshness, and constraints, not hit rate alone"

---

## 合意した方針変更

### 1. ベースライン拡充 (最優先)

```text
現在: LRU → GDSF → EconomicGreedy → (optional: Belady, ILP)
目標: LRU → GDSF → SIEVE → S3-FIFO → EconomicGreedy → (optional: Belady, ILP, W-TinyLFU)
```

実装順: SIEVE → S3-FIFO → W-TinyLFU → (LRB は後回し)

### 2. Synthetic α デフォルト変更

- デフォルト: `0.8` → `0.6`
- 実験標準レンジ: `[0.3, 0.5, 0.7, 0.9]`

### 3. SA 名称修正

- 現在: "QUBO solver with simulated annealing"
- 変更後: "Quadratic constrained SA" (capacity reject方式)
- Lucas型 penalty-QUBO は将来課題として分離

### 4. 差別化ポイントの修正

1. Hit rate ではなく **objective-aware optimization**
2. Size / origin cost / stale risk / latency value を **同一目的関数で統合**
3. **制約を明示** できる
4. **解釈可能** で ILP/Belady に近い検証ができる

---

## 次の開発ステップ (確定)

1. ~~`SIEVE` baseline を qc-simulate に追加~~ ✅ Done
2. ~~`S3-FIFO` baseline を qc-simulate に追加~~ ✅ Done
3. ~~`qc compare` に新ベースラインを載せる~~ ✅ Done
4. ~~Synthetic Zipf α デフォルトを 0.6 に変更~~ ✅ Done
5. ~~README/docs の SA 表現を修正~~ ✅ Done
6. Admission gate 設計と threshold calibration → V2.5
7. W-TinyLFU baseline → V2.5

---

## Reviewer Feedback (2026-04-02 追記)

### 指摘内容

> EconomicGreedy は Hit%, ByteHit%, CostSavings$, Objective$ の全てで
> SIEVE/S3-FIFO に負けている。「何を最適化したいのか」は正しくなったが、
> 「その目的でも勝てているのか」はまだ別問題。

### 根本原因分析

EconomicGreedy は **static offline policy**（固定集合を選択）。
SIEVE/S3-FIFO は **online adaptive policy**（動的に evict/admit）。
容量 1MB で 500 objects のうち 58 個しか固定選択できない。
SIEVE は時間とともに 200+ objects をローテーションできる。
→ 構造的に static policy が online policy に勝てない。

### Codex との議論結果

**Option A を採用: Economic Scoring を Admission Policy として再定義**

- quant-cache の強みは「which objects are economically worth caching」の判定
- 弱みは「when to evict / rotate」の時間適応
- → admission gate + SIEVE/S3-FIFO eviction のハイブリッド

### 実装状況

- `EconomicAdmission` gate: 実装済み
- `EconSievePolicy` / `EconS3FifoPolicy`: 実装済み
- ただし **threshold=0（全通過）では pure SIEVE と同一結果**
- **threshold > 0 では admission が厳しすぎて hit rate/objective が低下**

### 今後の課題

admission threshold の最適設計は V2.5 課題として保留。
現時点では:
- SIEVE/S3-FIFO 自体が強い one-hit-wonder filter を内蔵
- Economic admission が上乗せで価値を出すには、workload 特性への適合が必要
- threshold calibration (train/val split) が必要

### 実験結果の見せ方 (レビュワー指摘対応)

| Approach | Status |
|----------|--------|
| EconomicGreedy が勝つ条件を明示する | 静的 prewarm/prefetch ユースケースで有効 |
| 平均/中央値/勝率で出す | 20-seed sweep を基本にする |
| Objective$ の定義を明確化する | docs に数式を記載 |
| 負けるケースを分析として見せる | static vs online の構造差として説明 |

### 正直な位置づけ

現時点の quant-cache の価値は:
1. **経済目的関数の定式化** — hit rate ではなく $/period で評価する枠組み
2. **ILP による最適性検証** — greedy がどれだけ optimal に近いかを定量化
3. **再現可能な trace replay 評価基盤** — SIEVE/S3-FIFO/GDSF を同一条件で比較
4. **GDSF の objective 弱点を可視化** — GDSF は hit rate は高いが stale penalty で objective が大幅マイナス

runtime eviction policy としては SIEVE/S3-FIFO が優位。
quant-cache の勝ち筋は admission policy / prefetch / 経済分析ツールとしての位置づけ。
