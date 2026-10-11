# DMAコピー・通知待ちガイド検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
文書ID：`verification-guide-dma-notification`  
公開状態：補助記録（読者用サイトの外）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-11` | DMAの独立実習の実測・Viewer・ブラウザ・fresh証跡負例を記録 |

基準mainは `25f74c8e24842485d22b64797f0f28c32253ad0f`。正式Release/tagを再取得し、cleanなv1.1.4 / `b45515644dfc65c205216d564d5bf7ef00280622` のバイナリをWSL Ubuntu 24.04で使用した。画像生成は使用せず、公開サンプルだけで実Viewerを撮影した。

- CLI：新規 `guide-evidence/dma-final` へ18操作。入力と隔離ZIPを各3条件validate/run/viewし、子要求・バイト列・時刻・親状態・通知・manifestの全ファイルhashを検証。JSON差分はnotify_psだけ、全simulation objectsのZIP再現一致を確認。
- Viewer：実Chromium149.0.7827.55、900psの親選択、6要求・カウンター・8バイト・通知状態をassert。公開PNG3枚の実画素を確認。
- サイト：MkDocs strict成功。15 HTML / 800ローカルリンク・画像・アンカー。実Chromiumで404以外の14ページを巡回し、390pxの幅、画像読込、日本語「コピー」「通知」の記事検索をassert。記事の先頭・実測表の390px PNGも実画素を確認。
- 誤利用回帰：有効な旧出力 `dma-verified` がある状態でも両browser checkerが引数省略・不存在・指定fresh Viewer欠落を失敗とする6負例を実行。freshパス指定は必須。旧出力へのfallbackなし。
- 配布：既存6 ZIPの内容を維持し、新規8入力ZIPを含む全7 ZIPの一致を確認。
- Traceability：回帰40件成功、全体strictおよびDIR-REQ-0210〜0213の適用範囲strict成功。要求・機能の追加なし。生成一覧はstage済み最終文書から生成しfreshness確認。
- ライセンス：回帰9件、guide原本8・供給payload37・lock package42・crate原本89・source bundle10の対応検証成功。法的承認を意味しない。

証跡は [measurements.json](measurements.json)、[viewer-browser.json](viewer-browser.json)、[site-browser.json](site-browser.json)。browser証跡は指定fresh出力の9ファイルhashを記録。raw出力・一時画面・ローカル環境パスはGit対象外。exact head CIとProject状態はDraft PRで確認する。

制約：実機DMA・CPU同期・AXI・割り込み・DDR・fault・競合アクセスの性能や正しさは未検証。Windows native CLI単独実行も未検証。通常の時間制限で通知が未実行の条件を成功通知として扱わない。マージ・auto-merge・Issue close・デプロイは実施しない。
