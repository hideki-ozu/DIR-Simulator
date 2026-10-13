# SoC調停実習検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
実測ソースcommit：`b45515644dfc65c205216d564d5bf7ef00280622`  
文書ID：`verification-guide-soc-arbitration`  
文書状態：補助記録（読者サイトの外）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-13` | SoC調停2条件・隔離ZIP・実Viewer・metadata/nav・サイト・回帰を検証 |

基準mainは `65e862d50626eb3c9e772cdbf6b404de944efc33`。正式Release/tagを再取得してclean v1.1.4のCLIをWSL Ubuntu 24.04で使った。v1.1.5はタグのみで正式Releaseとして扱わない。既存記事・全Issue/PRと進行中作業を照合してSoC調停との重複がないことを確認した。AHB/NoCには着手しない。

- CLI：新規 `guide-evidence/soc-final` に12操作。公開入力と隔離ZIPで2条件をvalidate/run/viewし、全simulation一致、arbitrationだけの差分、3要求・3transferの対応、順序/FIFO・時刻・応答・bit・busy・manifestを照合。構成node4/channel0、gate接続6本。
- 結果：RRはa:0→b:0→a:1、固定はa:0→a:1→b:0。start0/30/60、completed30/60/90ps、全部OKAY、192bit・busy90ps。全体平均待ち30、m0平均30/15、m1平均30/60ps。T100、last_event90、events_exhausted、partial=false、残イベント0。依頼時の予測と独立実測は一致した。
- 実Viewer：95ps、b:0のstart/complete/OKAY、3要求・完了3を実ブラウザでassert。PNG2枚の実画素を開いて確認。表示の横並びはNEDの正確な配線図と扱わない。画像生成は使わない。
- fresh証跡：両browser checkerへsoc-finalを明示し選択6ファイルhashを保存。別の有効な旧soc-verifiedがあっても引数省略・不存在・指定Viewer欠落を拒否する6負例に合格。
- サイト：MkDocs strict、19 HTML / 1100ローカル参照、管理メモ除外。実ブラウザで404以外18ページ、390px・全画像読込・外部要求なし、検索「調停」「待ち」から新記事へ到達を確認。正確なbrowser版はJSONに保存する。desktop/390pxの画素も確認する。
- 配布：既存10 ZIPを保持し、新規6入力ZIPを含む全11 ZIPの入力一致を検証。
- metadata/nav：実生成器のパーサーで源文書と生成Markdown/HTMLの新規2文書版・v1.1.4を検査する。正常→commit併記拒否→復元の負例と、nav18＝16記事＋入口/ライセンス2・報告18と集合一致・404込みHTML19を検査する。
- traceability：回帰40、全体strict、DIR-REQ-0186〜0189適用strict、231要件/60機能/274ノード・欠落0。最終文書stage後に一覧を生成し鮮度を確認する。
- ライセンス：回帰19件、guide原本8・供給payload37・現lock44/歴史lock42/追加2・crate原本93・source bundle10を照合。法的承認を意味しない。

[measurements.json](measurements.json)、[viewer-browser.json](viewer-browser.json)、[site-browser.json](site-browser.json)に公開証拠を保存する。raw出力・一時サイト画面・個人環境パスはGit対象外。exact head CIとProject非完了状態はDraft PR本文に記録する。

制約：有限3要求・target1・正常系だけ。連続負荷/飢餓回避・容量超過・ERROR・複数target・非edge生成・打切り・実機性能・native・データbyte・CPU/cache・RTL/電気的適合は未検証。マージ・auto-merge・close・タグRelease変更・デプロイはしない。
