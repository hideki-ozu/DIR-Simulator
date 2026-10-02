# OMNeT++とのCAN比較

文書バージョン：`0.1.1`
対象GitHubバージョン：`v0.1`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.1` | `2026-10-03` | Classical CANのCLI・ライブラリ、結果ビューア、実行例、OMNeT++比較の利用方法を集約し、v0.1公開版を確定 |

既存のOMNeT++ / FiCo4OMNeTを実行し、DIRと同じ生入力から得た結果を比較する検証用ツールです。DIR製品へのOMNeT++コードの組み込みや、DIRの結果を正解値として再生する処理は行いません。

```bash
cargo build --release --locked -p dir-simulator
python3 tools/omnet_comparison/compare.py \
  --omnet-workspace /home/hideki/Omnet++ \
  --output tmp/omnet-comparison-new
```

出力先は新規ディレクトリを指定してください。インストール済みワークスペースの`scripts/env.sh`、OMNeT++開発ツール、ビルド済み`upstream/FiCo4OMNeT/src/libFiCo4OMNeT.so`を使用します。既存のランタイム・ライブラリ・設定は変更せず、[観測アダプター](model/README.md)だけを出力先にビルドします。

`--case competition --case overload`のように個別ケースも選べます。`--dir-binary`で比較するDIR実行ファイルを指定できます。

## 対象

| 区分 | ケース数 | 入力元 |
|---|---:|---|
| CANシナリオ | 8 | `docs/verification/fixtures/can/scenarios.json`と対応INI/JSON/NED |
| 負荷例 | 3 | `examples/can/{baseline,contention,overload}.ini` |
| 混在ID・観測境界 | 5 | `can_scenarios.rs`の混在ID2条件、H=0、生成器なし、期間外のみ生成 |
| フレーム時間 | 27 | `vectors.json`の9フレームを500000/1000000/333333bpsで実行 |

合計43ケース。境界条件の一部はRustライブラリ試験の条件をINI/workloadに表現した追加実行です。CRCやビット列の検証そのものはFiCoでは行えません。DIR独自の入力拒否・イベント数制限・u64オーバーフロー・結果出力・ビューア試験、未実装の将来モデルについては、報告書の対象外一覧で理由を示します。

## 比較の意味

送信時刻、CAN ID・形式、payloadバイト列、ビットレート、ノード数、観測期間を元ファイルから独立に変換します。元のNED/INIをOMNeT++にそのまま渡すものではありません。対応していない設定や配線はエラーとし、黙って省略しません。

FiCoにない有限キュー容量、DIRで指定したTX/RX処理遅延・配送遅延・受信フィルタはアダプターで補います。既存FiCoのバス・仲裁・ポート・時間計算は変更しません。FiCoは固定stuff率によるフレーム長近似、アイドル時の1bit待機、IFSを含む送受信完了というモデルなので、DIRと実行結果が一致するとは限りません。固定stuff率は0とし、結果を合わせる調整は行いません。

生成イベントだけでなく、SOFから受信までの全イベントでID・形式・実payload・送信元の不変性を検証します。要求・受信先ごとの重複や不正な遷移、キュー件数の不整合、遅延条件の不一致を検出してから集計します。すべての時刻はps整数、占有・キュー積分は整数／有理数で比較します。

`native_complete`はFiCoでのIFS込み完了です。DIRの`eof_ps`と同じ意味にせず、別の列に保存します。バス占有終了の比較にはDIRの`release_ps`とFiCoの`native_complete`を使います。`success`件数はそれぞれのモデルの完了定義に従うため、特に期間終端付近では注意が必要です。

## 出力と判定

v0.1公開時の43ケースの結果と独立計算による差分評価は、[OMNeT比較結果](../../docs/verification/OMNeT比較結果.md)に記録しています。全ケースの入力・生結果・集計も同文書から取得できます。

- `report.md` / `report.json`: ケース別結果、入力条件、対応外項目、実行版・SHA-256。
- `cases/<case>/inputs/`: 実行時に固定したDIRの入力。
- `cases/<case>/dir/results.json`: DIRの生結果。
- `cases/<case>/omnet/`: OMNeT入力、生成要求一覧、`events.csv`、ログ、標準sca/vec。
- `cases/<case>/comparison.json`: 全要求・受信、件数、送信順、キュー、占有時間、差分。
- `cases/<case>/requests.csv`: DIRのSOF/EOF/releaseとFiCoのSOF/native完了を並べた表。

`DIFF`は実測差を表し、比較処理の失敗ではありません。`EQUAL`も共通観測項目の一致であり、モデルや全APIの同等性を意味しません。実行エラー、入力対応や生イベントの整合性に問題がある場合は非ゼロで終了し、ケースの`error`に記録します。

比較器のテスト:

```bash
python3 -m unittest discover -s tests -p test_omnet_comparison.py
```

OMNeT++のイベント優先順位・固定小数点時刻については[公式マニュアル](https://doc.omnetpp.org/omnetpp/manual/)を参照してください。使用したFiCoコードは実行ごとのハッシュで特定します。
