# Issue #53 区間比較の検証

文書バージョン：`1.0.0`
対象GitHubバージョン：`main @ 87740a7`
文書ID：`verify-viewer-intervals-2026-10-11`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-11` | Issue #53 の承認済み定義、実装接続とモデル・実画面・文書の検証結果を記録 |

基準 main: `87740a7fc83d30c652d3a64e98ec4cd1252a8610`。独立ブランチ `viewer-interval-comparison-53`。Issue #52 のユーザーによるマージ後の main から開始した。

承認済み定義は半開区間、未完了の観測終了打切り、非空合計・最長の併記、同時刻最終状態の最大、開始以前の計測欠如 N/A。全記録を集計する純粋モデル→区間選択・別表・並び替え→既存再演・ノード/要求詳細の順に接続した。新規判断待ちはない。

## ローカル実行

- `node --test --test-skip-pattern=actual tests/viewer_model.test.cjs`: 27/27 PASS。既存23と追加4。整数 ps ごとの独立 oracle は短区間の全開始・終了組合せで合計・最長・最大を比較。1,240要求の2バスは既知の100スロット占有時間と照合。
- `node tests/viewer_browser.cjs --interval-comparison`: PASS。実 Chromium で schema1/2 と公開保存済み CAN 結果を操作。時刻入力、巻き戻し、区間/並び順、RX/TX分離、1,240要求、ページ/検索独立、ゼロ長、390px、ファイル/不正入力リセット、元公開結果バイト不変、外部 HTTP(S) 通信0を確認。
- `node tests/viewer_browser.cjs --recorded-events`: PASS。Issue #52 のイベント移動・再生 focus・ステップ取消・u64・各 profile 切替の回帰。
- ブラウザの初回実行は試験側が存在しない `jump-unit` を参照して失敗。既存 `time-unit` / `jump` を使用するよう修正し、再実行が PASS。製品コードの回避・機能削除は行っていない。

ローカルに cargo / Rust CLI がないため actual CLI 2試験と全 browser run はローカルでは未実施。Linux リモート Rust workflow で全29モデル試験と全ブラウザ回帰を実行する。厳密なリモート head と CI 結果は PR に記録する。要求チェック名、ruleset、標準自動マージ設定は変更しない。

狭い画面の [比較表スクリーンショット](comparison-mobile.png) を保存し、比較表の横スクロールとページ全体の幅を目視確認した。`mkdocs build --strict` PASS、ガイドの13 HTML / 661リンク PASS、traceability 231要件 / 60機能 / 274ノード / 構造エラー0、traceability unit tests 40/40 PASS。新規報告の文書ID欠如は追加して修正した。生成鮮度の最終検証とリモート CI は PR に記録する。schema、Runtime、出力生成、元 manifest の変更なし。
