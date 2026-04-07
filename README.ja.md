# quant-cache

[English](README.md) | **日本語**

CDN 運用者のための経済的キャッシュ意思決定フレームワーク。

quant-cache は **経済目的関数** ($/期間) を通じてキャッシュポリシーを評価します。
レイテンシ削減、オリジンコスト削減、鮮度ペナルティを単一の指標に統合し、
ヒット率だけの評価では見えないコストを可視化します。

例えば、GDSF は最も高いヒット率を達成しますが、stale コンテンツへのペナルティにより
**経済目的関数がマイナス** になります。

quant-cache は SIEVE や S3-FIFO のような eviction ポリシーの代替ではありません。
**意思決定・評価レイヤー** として、以下の問いに答えます:
- どのオブジェクトをキャッシュすることが経済的に価値があるか？
- 鮮度コストを考慮した場合、キャッシュポリシーの性能はどうなるか？
- Greedy ヒューリスティックは数学的最適解にどれだけ近いか？

## 主要な発見

20 本の synthetic trace で評価 (Zipf α=0.6, 500 objects, 50k requests):

| ポリシー | Objective$ (平均) | Hit% (平均) | CostSavings$ (平均) |
|---------|-------------------|-------------|---------------------|
| **SIEVE** | **392.57** | 36.4% | 361.74 |
| S3-FIFO | 380.48 | 35.1% | 350.79 |
| LRU | 324.90 | 35.0% | 349.66 |
| **GDSF** | **-133.19** | **44.1%** | **562.64** |

GDSF はヒット率・コスト削減額ともに最高ですが、**経済目的関数は大幅なマイナス** です。
更新頻度の高いオブジェクトをキャッシュすることで stale ペナルティが蓄積するためです。

## クイックスタート

```bash
cargo build --workspace --release

# synthetic trace を生成
qc generate --num-objects 10000 --num-requests 1000000 --output trace.csv

# 経済評価でポリシーを比較（admission threshold を自動キャリブレーション）
qc compare --input trace.csv --capacity 50000000 --preset ecommerce

# CDN ログをインポート (CloudFront, Cloudflare, Fastly)
qc import --provider cloudfront --input access.log --output trace.csv
qc import --provider cloudflare --input els.ndjson --output trace.csv
qc import --provider fastly --input realtime.ndjson --output trace.csv

# 経済的に最適なキャッシュセットを見つける
qc optimize --input trace.csv --output policy.json --capacity 50000000 --preset ecommerce

# ポリシー構成を探索 (grid, SA, QUBO)
qc policy-search --input trace.csv --capacity 50000000 --method sa --output best-ir.json

# 経済パラメータを自動調整
qc calibrate --train train.csv --validation val.csv --capacity 50000000
```

## エンドツーエンド: トレース → CDN 設定

```bash
# 1. ログをインポート (または synthetic を生成)
qc import --provider cloudfront --input access.log --output trace.csv

# 2. 最適なポリシー構成を探索
qc policy-search --input trace.csv --capacity 50000000 \
  --preset ecommerce --method sa --output best-policy.json

# 3. ポリシーをトレースで評価
qc policy-eval --input trace.csv --policy best-policy.json --preset ecommerce

# 4. admission gate 用のスコアを生成
qc optimize --input trace.csv --output scores.json \
  --capacity 50000000 --preset ecommerce

# 5. 任意の CDN ターゲットにコンパイル + 検証
qc compile --policy best-policy.json --scores scores.json \
  --target cloudflare --output cloudflare-config.json --validate
qc compile --policy best-policy.json --scores scores.json \
  --target cloudfront --output cloudfront-config.json --validate
qc compile --policy best-policy.json --scores scores.json \
  --target fastly --output fastly-config.json --validate
qc compile --policy best-policy.json --scores scores.json \
  --target akamai --output akamai-config.json --validate

# 6. 全 CDN 出力を横断比較
qc compile-compare --policy best-policy.json --scores scores.json

# 7. デプロイ前安全性チェック
qc deploy-check --input trace.csv --policy best-policy.json --preset ecommerce
```

## 仕組み

### 1. 経済スコアリング

各キャッシュオブジェクトについて、時間窓 T における期待経済便益を計算:

```
benefit  = E[requests] × (latency_saving × λ_latency + origin_cost)
freshness_cost = E[requests] × P(stale) × stale_penalty    (TTL-Only モデル)
net_benefit = benefit - freshness_cost
```

2つのスコアリングバージョン:
- **V1 (頻度ベース)**: キャッシュされた場合、全リクエストがヒットすると仮定
- **V2 (再利用距離)**: `p_hit = exp(-rd_p50 / cache_capacity_objects)` — 時間的局所性で割引

### 2. リプレイ評価

7 つのポリシー (LRU, GDSF, SIEVE, S3-FIFO, Belady, EconSieve, EconS3FIFO) に対してトレースをリプレイし、
従来の指標と経済目的関数の両方を計測します。

### 3. 限界付き最適性

GreedySolver (O(n log n)) で 0-1 ナップサック問題を解き、ExactIlpSolver で検証。
観測された最適性ギャップ: **中央値 0.01%, p95 0.72%** (n=1000, 50ケース)。

### 4. パラメトリック検証

2,880 以上のパラメータスイープで不変量を検証:
- Belady ヒット率 ≥ 全オンラインポリシー
- ソルバーが容量制約を遵守
- NaN/Inf が伝搬しない
- 容量増加に対して目的関数が単調非減少
- 決定的な結果

## CLI コマンド

| コマンド | 説明 |
|---------|------|
| `qc import` | CDN ログを canonical trace CSV に変換 (CloudFront, Cloudflare, Fastly) |
| `qc generate` | 設定可能な分布で synthetic trace を生成 |
| `qc optimize` | 経済的に最適なキャッシュセットを探索 (greedy, ILP, SA solver) |
| `qc simulate` | 保存されたポリシーでトレースをリプレイ |
| `qc compare` | LRU, GDSF, SIEVE, S3-FIFO, Belady を経済指標で並列比較 |
| `qc calibrate` | train/validation 分割で経済パラメータを自動調整 |
| `qc policy-eval` | PolicyIR 構成をトレースで評価 |
| `qc policy-search` | backend/admission/bypass/prewarm 空間を探索 (grid/SA/QUBO) |
| `qc compile` | デプロイメント設定を生成 + 検証 (Cloudflare/CloudFront/Fastly/Akamai) |
| `qc deploy-check` | デプロイ前安全性チェック (LRU/SIEVE 比較 + 閾値検証) |
| `qc compile-compare` | 同一 PolicyIR を全 4 プロバイダにコンパイルして比較 |

## ベースライン

| ポリシー | タイプ | 出典 |
|---------|--------|------|
| LRU | オンライン eviction | 古典 |
| GDSF | オンライン eviction (コスト考慮) | Cao & Irani, 1997 |
| SIEVE | オンライン eviction (遅延昇格) | Zhang et al., NSDI 2024 (Best Paper) |
| S3-FIFO | オンライン eviction (3キュー) | Yang et al., SOSP 2023 |
| Belady | オフラインオラクル (未来知識) | Belady, 1966 |
| EconSieve | SIEVE + 経済的 admission gate | quant-cache |
| EconS3FIFO | S3-FIFO + 経済的 admission gate | quant-cache |
| EconomicGreedy | オフラインナップサック選択 | quant-cache (Dantzig, 1957) |
| ExactILP | オフライン最適解 | HiGHS solver |
| SA/QUBO | オフライン二次 (co-access 相互作用) | quant-cache |

## アーキテクチャ

```
quant-cache/
├── crates/
│   ├── qc-model/      データ型、設定、プリセット、経済パラメータ
│   ├── qc-solver/     Scorer trait (V1/V2), Greedy/ILP/SA ソルバー, QUBO DSL, calibration
│   ├── qc-simulate/   リプレイエンジン、7+ baseline ポリシー、synthetic generator, co-access
│   └── qc-cli/        CLI (11 コマンド), 3 ログパーサー, 4 CDN コンパイラ
├── docs/              設計文書、ロードマップ、関連研究 (29 論文)
└── CHANGELOG.md       リリース履歴
```

~8,200 行のソース + ~5,100 行のテスト、31 テストスイート。

## テスト

```bash
cargo test --workspace                            # 170+ テスト (smoke tier, <30s)
cargo test --release --workspace -- --ignored     # 完全パラメトリック検証 (2,880+ パターン)
cargo clippy --all-targets -- -D warnings         # lint
cargo fmt --check                                 # format
```

## ロードマップ

| バージョン | フォーカス | 状態 |
|-----------|----------|------|
| V1.0 | 経済ナップサック + trace replay | 完了 |
| V1.1 | CloudFront ログインポート | 完了 |
| V1.5 | Belady オラクル、キャリブレーション | 完了 |
| V1.6 | 再利用距離スコアリング (V2) | 完了 |
| V2.0 | 二次 SA、co-access 相互作用 | 完了 |
| Phase B | Policy IR + IR evaluator | 完了 |
| Phase C | ポリシー探索 (grid/SA/QUBO) | 完了 |
| Phase D | 4 CDN デプロイメントスキャフォールド | 完了 |
| Phase E | パラメトリック検証 + QUBO DSL | **完了** |

## 学術的背景

quant-cache は 29 本の論文に基づいています: 古典アルゴリズム (Belady, GDSF)、
最新 eviction (SIEVE, S3-FIFO, TinyLFU)、ML アプローチ (LRB, CACHEUS)、
最適化理論 (Dantzig ナップサック, Lucas QUBO)、本番システム (CacheLib)。

詳細: [docs/related-work.md](docs/related-work.md)

## コントリビュート

[CONTRIBUTING.md](CONTRIBUTING.md) をご覧ください。

## ライセンス

[MIT](LICENSE)
