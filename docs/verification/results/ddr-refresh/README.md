# DDRリフレッシュガイド検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
文書ID：`verification-guide-ddr-refresh`  
文書状態：補助記録（読者サイトの外）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-11` | DDRの1変数実測・隔離ZIP・実Viewer・fresh証跡負例・サイト検証を記録 |

基準mainは `f4f284c9ece09427baea5e6448169770d561d728`。公開Release/tagを再取得し、clean v1.1.4 / `b45515644dfc65c205216d564d5bf7ef00280622` の正式ソースからビルド済みCLIをWSL Ubuntu 24.04で使用した。正式版とmainは区別した。画像生成は使わず、公開サンプルの実Viewerを撮影した。

- CLI：新規 `guide-evidence/ddr-final` に18操作。公開入力と隔離ZIPを3条件それぞれvalidate/run/viewし、全simulation一致、3要求の時刻・row_hit・bytes・refresh実績・queue・busy・manifest全ファイルhashを検証。差分は `ddr[0].refresh_ps` だけ。
- 実測：初回read0→200ps、2回目450→550psは不変。refreshの開始は550ps、終了600/650/750ps、3回目の開始も600/650/750ps、完了800/850/950ps。全readは01020304、completed3/rejected0、refresh1、busy_ratio0.5、queue_mean0.1/0.15/0.25。全条件T1000psで正常time_limit、partial=false、残イベント1。
- Viewer：実Chromium149.0.7827.55で999ps、3要求・完了3、read:2の詳細と資源欄refresh実績をassert。公開PNG3枚の実画素を開いて確認。
- サイト：MkDocs strict、16 HTML / 875ローカルリンク・画像・アンカー、公開管理メモ除外。実Chromiumで404以外の15ページ、390px全体幅、全画像読込、外部要求なし、日本語「リフレッシュ」「DDR」から記事へ到達をassert。記事desktop先頭・390px先頭/実測表の実画素も確認。
- Fresh証跡：両checkerへ `ddr-final` を明示。別の有効な旧出力 `ddr-verified` が存在しても、引数省略・不存在・fresh Viewer欠落を失敗とする6負例に合格。選択9ファイルのhashをbrowser報告へ保存。
- 配布：既存7 ZIPを維持し、新規8入力ZIPを加えた全8 ZIPの入力一致を確認。
- Traceability：回帰40件、全体strict、DIR-REQ-0198〜0201の適用範囲strict成功（231要件・60機能・274ノード、欠落0）。新規要求/機能なし。生成一覧は最終版のstage後に生成しfreshnessを確認する。
- ライセンス：回帰9件、guide原本8・供給payload37・lock package42・crate原本89・source bundle10の対応検証成功。法的承認を意味しない。

[measurements.json](measurements.json)、[viewer-browser.json](viewer-browser.json)、[site-browser.json](site-browser.json)に公開証拠を保存する。raw出力・一時サイト画像・個人環境パスはGit対象外。exact head CIとProject状態はDraft PR本文に記録する。

制約：実機DDR・bank並列・JEDEC全体、書き込み・別行/bank・fault・満杯・DMA競合・refresh途中打切りは未検証。Windows native CLI・CPU/cache/AXI/OS同期・性能試験も未実施。予定イベントを実績扱いしない。マージ・auto-merge・close・デプロイは実施しない。
