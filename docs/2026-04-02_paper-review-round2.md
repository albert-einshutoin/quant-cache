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

1. `SIEVE` baseline を qc-simulate に追加
2. `S3-FIFO` baseline を qc-simulate に追加
3. `qc compare` に新ベースラインを載せる
4. Synthetic Zipf α デフォルトを 0.6 に変更
5. README/docs の SA 表現を修正
6. α sweep による感度分析
7. W-TinyLFU baseline (後回し)
