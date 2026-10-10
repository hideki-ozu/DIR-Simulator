# CAN出力最適化①〜⑤の製品統合・実測記録（2026-10-08）

文書ID：`can-output-integrated-2026-10-08`

①バイナリ外部ソート、②観測点・時間窓の2系列マージ、③JSON／CSVの単一走査、④固定長寄与・Timeline添字参照、⑤Stats添字参照を製品コードへ統合した。[詳細設計](../../design/CAN出力最適化詳細設計書.md)と[検証仕様](../cases/結果処理検証仕様書.md#dir-test-0117)も更新した。公開schema・指標値・行順・採番の契約を維持する。

## 統合版の直接比較

同じ固定基準版のバイナリと入力を使い、今回の統合版と改めて比較した。個別試作の短縮率を加算していない。各条件・各版はウォームアップ1回と本測定3回で、実行順をroundごとに反転し、全24実行を逐次実行した。比較中にビルド・CPUプロファイリング・別のベンチマークを重ねていない。GNU timeのwallはCLIの準備から出力完了まで、最大RSSは同じCLIプロセスの値である。出力hash検証時間はwallの外にある。

時間は3測定の中央値、RSSは3測定の最大値を示す。

| 条件 | 基準wall | 統合wall | 時間短縮 | 基準最大RSS | 統合最大RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| 16,000要求・rho 0.30 | 32.37秒 | 10.28秒 | 68.2% | 116.13 MiB | 47.12 MiB |
| 3,200要求・rho 0.90 | 3.63秒 | 1.24秒 | 65.8% | 101.98 MiB | 39.61 MiB |
| 3,200要求・rho 1.20 | 3.12秒 | 1.03秒 | 67.0% | 110.18 MiB | 38.06 MiB |

| 条件 | 基準wallの3生値 | 統合wallの3生値 |
| --- | --- | --- |
| 16,000要求・rho 0.30 | 32.13, 32.37, 32.48 | 10.13, 10.31, 10.28 |
| 3,200要求・rho 0.90 | 3.60, 3.81, 3.63 | 1.26, 1.24, 1.23 |
| 3,200要求・rho 1.20 | 2.88, 3.16, 3.12 | 1.04, 1.03, 1.03 |

生RSS、範囲、中央値差、測定roundを対応付けた差はJSONに記録した。3測定から微小差の統計的有意性を断定しない。

## 正しさと公開処理

Rust 1.85.0でfmt、locked/offline workspace Clippy（warnings禁止）、locked/offline workspace test、release buildが通過した。統合版は543 passed／0 failed／0 ignored、28 suites。個別案の試験に加え、バイナリ形式と遅い全行fallback、同一キーの点優先と再走査、直接／fallback双方の生成frame長上限、paired出力の後半行破損・cleanup、複数バスと無通信対象におけるStats／Timelineの対応を検証した。材料化した既存結果とのJSON simulation／CSV比較も通過した。

全24実行で各4ファイル、計96ファイルの実byte数・SHA-256をmanifest宣言値と照合した。条件内の8実行すべてでsimulation JSON、run_idを正規化したevents.csv／summary.csv、およびdiagnostics.jsonlのhashが一致した。実行ID・日時・ビルド情報を含むresults.json全体のraw hashは版ごとに変わる。

静的統合レビューで、整列済み直接経路とfallbackへの生成frame上限の伝播、fallbackによる保存済み全行の再投入、集計の非再実行、完全キーの点優先、寄与のtime＋tie順、Timeline IDとStats slotの分離、両writer完了後の登録とmanifest最終公開を確認した。OSのflush／close自体に障害を注入した証明は今回の追加試験に含めない。

## ソース対応と証跡

基準183ファイルと統合184ファイルのhash mapを固定し、変更は出力層の7ファイル（うちcontribution_sort.rsは追加）に限定した。他の製品ソースは今回の開始時点と一致する。イベント実行・入力・台帳生成を変更していない。測定前後のソース・バイナリ・入力・計測スクリプトのpinを照合した。統合CLI SHA-256は`1e134d589885a593d295771d955b0e59c1f7e0d468f83317b25fc21da4b09de7`、基準CLIは`afa99e235603c1cf2cac6494ce90dd0c2b737457f88c4ba91c11042e51734c54`。

[機械可読記録](can-output-integrated-2026-10-08.json)と同名supportディレクトリにgateログ、測定controller、全24実行のGNU time／attempt／サンプル、入力生成記録と入力、基準ソース、統合差分とpatch、再現用基準snapshotを保存した。`support_relative_path`は`inventory_base_directory`から解決し、`original_local_path`は保存元のローカル位置を示す。実行バイナリはローカル`/tmp/dir-opt-integrated-2026-10-08/bin/`に保持し、製品リポジトリへコピーしていない。

保存した389ファイルのinventory、183→184ファイルのソース対応、patchの独立再適用、24回の生GNU time／attempt／集計値、96個のmanifest照合entryを独立検証し、すべて通過した。[独立照合記録](can-output-integrated-2026-10-08/validation/evidence-integrity-verification.json)に保存する。照合対象の元JSONもhash付きで保持し、その後に照合スクリプトと結果を添付した。JSONの`paired_effect_claim: false`は統計的な対応比較による推論を主張しないという意味であり、①〜⑤を併用した直接測定は実施済みである。

大きな結果ファイルと元のmanifestファイルは検証後に削除した。元の4ファイル分のmanifest宣言entryと観測hash／byte数はattempt記録内に保存している。基準snapshotと統合patchを使って測定ソースを復元できる。既存の個別試作記録は当時の製品未適用状態を表す履歴として維持する。

## 検証範囲

①〜⑤の併用効果を今回直接測定した。フル100万要求の完走・120秒目標・全体peak RSSの受け入れは今回実施していない。小規模測定の短縮率を100万要求へ外挿しない。
