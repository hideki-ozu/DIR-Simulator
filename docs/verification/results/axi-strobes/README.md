# AXIストローブ実習検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
実測ソースcommit：`b45515644dfc65c205216d564d5bf7ef00280622`  
文書ID：`verification-guide-axi-strobes`  
文書状態：補助記録（読者サイトの外）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-12` | AXIのWSTRB比較・隔離ZIP・実Viewer・metadata/nav件数・サイト検証を記録 |

基準mainは `dbcbcc549e737c6dd68d544bf61f53ce5811bba7`。正式Release/tag v1.1.4を再取得し、clean正式CLIをWSL Ubuntu 24.04で使用した。v1.1.5はタグのみでRelease未公開として区別した。PR67は別の共有メモリ記事、PR73はNED run/Viewer修正で、本記事と同じ製品ソースの編集はしていない。共通文書の将来の統合では各記事・履歴を保持する必要がある。

- CLI：新規 `guide-evidence/axi-final` に18操作。公開入力と隔離ZIPの各3条件をvalidate/run/viewし、全simulation一致、差分WSTRB一値、5 handshake・VALID標本・時刻・応答・RAM・read_data・bit集計・manifest全ファイルhashを確認。構成はnode3/channel0、gate接続10本。
- 実測：WSTRB0/5/15のreadは00010203/11013303/11223344。write0→30ns、read50→70ns、AW10/W20/B30/AR60/R70ns、OKAY2、生成/完了2、written_bits0/16/32、read_bits32、bus_utilization0.5。全条件終了100ns、events_exhausted、partial=false、残イベント0。
- 初期のNEDはRAM Rポート向き不正で拒否され、修正後に実測を行った。画面検証では既存Playwrightモジュールが欠落していたため、専用ignored領域へPlaywright1.64.0を用意し、GUIDE_CHROMIUMで既存実Chromiumを指定した。依存欠落・画像未生成による初回strict失敗は、画像生成後の再検証と区別する。実ブラウザの正確な版はJSONへ保存する。
- Viewer：実ブラウザ90nsでread:0のdata/OKAY/完了時刻、最終RAM、counter2をassert。新規PNG3枚の実画素を開いて確認。AXI詳細は複数のJSON表示を持つため、最初のpreにあるtransactionと後続handshakeを分けて検査する。
- サイト：画像生成後にMkDocs strict成功、17 HTML、948ローカル参照を確認。実ブラウザで404以外16ページ、390px・全画像読込・外部要求なし、検索「ストローブ」「部分」を確認し、desktop/390pxの実画素を確認する。
- ZIP：既存8 ZIPを保持し、新規8入力ZIPを含む全9 ZIPの一致を確認。
- fresh証跡：両checkerにaxi-finalを明示し選択9ファイルhashを保存。旧axi-verifiedがあっても、省略・不存在・Viewer欠落を拒否する6負例を確認。
- metadata/件数：実生成器のパーサーで新規2文書の源文書と生成Markdown/HTMLの版・v1.1.4を照合する。nav16件＝14記事＋入口/ライセンス2、報告16件と集合一致、404込みHTML17件を検査する。
- Traceability：回帰40件、全体strict、REQ0133〜0144の適用範囲strict、stage後の生成一覧freshnessを確認する。製品機能や要求を追加しない。
- ライセンス：回帰19件とguide原本対応を確認。法的承認を意味しない。

[measurements.json](measurements.json)、[viewer-browser.json](viewer-browser.json)、[site-browser.json](site-browser.json)に公開証拠を保存する。raw出力・ローカルパス・一時サイト画像はGit対象外。最終head CIとProject非完了状態はPR本文に記録する。

制約：整列1 beat・Manager1・正常アドレス・固定READY。error・容量超過・競合・複数beat/Manager・途中打切り・native・RTL/電気的適合・実機性能は未検証。画像生成・マージ・auto-merge・close・タグRelease変更・デプロイは行わない。
