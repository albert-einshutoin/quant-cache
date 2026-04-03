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
これは明示的な経済モデルなしには見えない問題です。

## クイックスタート

```bash
cargo build --workspace --release

# synthetic trace を生成
qc generate --num-objects 10000 --num-requests 1000000 --output trace.csv

# 経済評価でポリシーを比較
qc compare --input trace.csv --capacity 50000000 --preset ecommerce

# CloudFront ログをインポート
qc import --provider cloudfront --input access.log --output trace.csv

# 経済的に最適なキャッシュセットを見つける
qc optimize --input trace.csv --output policy.json --capacity 50000000 --preset ecommerce

# 経済パラメータを自動調整
qc calibrate --train train.csv --validation val.csv --capacity 50000000
```

## エンドツーエンド: トレース → Cloudflare 設定

```bash
# 1. CloudFront ログをインポート (または synthetic を生成)
qc import --provider cloudfront --input access.log --output trace.csv

# 2. 最適なポリシー構成を探索
qc policy-search --input trace.csv --capacity 50000000 \
  --preset ecommerce --output best-policy.json

# 3. ポリシーをトレースで評価
qc policy-eval --input trace.csv --policy best-policy.json --preset ecommerce

# 4. admission gate 用のスコアを生成
qc optimize --input trace.csv --output scores.json \
  --capacity 50000000 --preset ecommerce

# 5. Cloudflare Rulesets API ペイロードにコンパイル
qc compile --policy best-policy.json --scores scores.json \
  --target cloudflare --output cloudflare-config.json --validate

# 6. デプロイ前安全性チェック
qc deploy-check --input trace.csv --policy best-policy.json --preset ecommerce
```

出力 `cloudflare-config.json` には以下が含まれます:
- Rulesets API ペイロード (`http_request_cache_settings` フェーズ)
- admission スコアが埋め込まれた Workers スクリプト
- プリウォーム URL リスト
- デプロイ手順

## 仕組み

### 1. 経済スコアリング

各キャッシュオブジェクトについて、時間窓 T における期待経済便益を計算:

```
benefit  = E[requests] × (latency_saving × λ_latency + origin_cost)
freshness_cost = E[requests] × P(stale) × stale_penalty    (TTL-Only モデル)
net_benefit = benefit - freshness_cost
```

全項目は $/期間 で統一。サイズ、アクセスパターン、更新頻度が異なるオブジェクト間で
コスト/便益を比較可能にします。

### 2. リプレイ評価

複数のポリシー (LRU, GDSF, SIEVE, S3-FIFO, Belady) に対してトレースをリプレイし、
従来の指標 (ヒット率, バイトヒット率) と経済目的関数 (レイテンシ価値 + オブジェクト別 stale ペナルティ) の両方を計測します。

### 3. 限界付き最適性

GreedySolver (O(n log n)) で 0-1 ナップサック問題を解き、ExactIlpSolver で検証。
観測された最適性ギャップ: **中央値 0.01%, p95 0.72%** (n=1000, 50ケース)。

## CLI コマンド

| コマンド | 説明 |
|---------|------|
| `qc import` | CDN プロバイダログ (CloudFront) を canonical trace CSV に変換 |
| `qc generate` | 設定可能な分布で synthetic trace を生成 |
| `qc optimize` | 経済的に最適なキャッシュセットを探索 (greedy, ILP, SA solver) |
| `qc simulate` | 保存されたポリシーでトレースをリプレイ |
| `qc compare` | LRU, GDSF, SIEVE, S3-FIFO を経済指標で並列比較 |
| `qc calibrate` | train/validation 分割で経済パラメータを自動調整 |
| `qc policy-eval` | PolicyIR 構成をトレースで評価 |
| `qc policy-search` | backend/admission/bypass/prewarm/TTL/key 空間を探索 |
| `qc compile` | デプロイメントスキャフォールドを生成 (Cloudflare/CloudFront) |
| `qc deploy-check` | デプロイ前安全性チェック (LRU/SIEVE 比較 + 閾値検証) |

## ベースライン

| ポリシー | タイプ | 出典 |
|---------|--------|------|
| LRU | オンライン eviction | 古典 |
| GDSF | オンライン eviction (コスト考慮) | Cao & Irani, 1997 |
| SIEVE | オンライン eviction (遅延昇格) | Zhang et al., NSDI 2024 (Best Paper) |
| S3-FIFO | オンライン eviction (3キュー) | Yang et al., SOSP 2023 |
| Belady | オフラインオラクル (未来知識) | Belady, 1966 |
| EconomicGreedy | オフラインナップサック選択 | quant-cache (Dantzig, 1957) |
| ExactILP | オフライン最適解 | HiGHS solver |

## プリセット

| プリセット | ユースケース | λ_latency ($/ms) | Stale ペナルティ |
|-----------|-------------|-------------------|-----------------|
| `ecommerce` | 商品ページ、カタログ | 0.00005 | High ($0.10/event) |
| `media` | 動画/画像配信 | 0.00001 | Low ($0.001/event) |
| `api` | REST API、認証トークン | 0.0001 | InvalidationOnUpdate |

Stale ペナルティコストは `StaleCostOverrides` で TOML 設定からクラス別にカスタマイズ可能です。

## アーキテクチャ

```
quant-cache/
├── crates/
│   ├── qc-model/      データ型、設定、プリセット、経済パラメータ
│   ├── qc-solver/     BenefitCalculator, GreedySolver, ExactIlpSolver, SA solver, calibration
│   ├── qc-simulate/   リプレイエンジン、5 baseline ポリシー、synthetic generator, co-access
│   └── qc-cli/        CLI (11 コマンド: import → generate → optimize → policy-search → compile → deploy-check)
├── data/samples/      サンプルトレースと設定ファイル
└── docs/              設計文書、関連研究 (29 論文)
```

## 学術的背景

quant-cache は 29 本の論文に基づいています: 古典アルゴリズム (Belady, GDSF)、
最新 eviction (SIEVE, S3-FIFO, TinyLFU)、ML アプローチ (LRB, CACHEUS)、
最適化理論 (Dantzig ナップサック, Lucas QUBO)、本番システム (CacheLib)。

詳細: [docs/related-work.md](docs/related-work.md)

## ロードマップ

| バージョン | フォーカス | 状態 |
|-----------|----------|------|
| V1.0 | 経済ナップサック + trace replay | 完了 |
| V1.1 | CloudFront ログインポート | 完了 |
| V1.5 | Belady オラクル、キャリブレーション | 完了 |
| V2.0 | 二次 SA、co-access | 完了 |
| Phase B | Policy IR + IR evaluator | 完了 |
| Phase C | ポリシー探索 (全 PolicyIR フィールド) | 完了 |
| Phase D | Cloudflare + CloudFront デプロイメントスキャフォールド | 完了 |
| Phase E | プロバイダスキーマ検証、量子探索 | 進行中 |

## テスト

```bash
cargo test --workspace                                # 100+ テスト
cargo test --release --workspace -- --ignored         # acceptance + perf guards
cargo clippy --all-targets -- -D warnings             # lint
```

## コントリビュート

[CONTRIBUTING.md](CONTRIBUTING.md) をご覧ください。

## ライセンス

[MIT](LICENSE)
