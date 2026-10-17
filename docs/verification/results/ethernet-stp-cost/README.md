# 抽象STPガイド検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-stp-cost`
文書状態：CLI・実Viewer・サイト検証合格。Projectは権限不足で未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-17` | 初版：L12 cost1変数・初期tree/role・全copy経路の正式版実測、隔離ZIP・実Viewerとサイト検証を追加 |

## 基準・来歴・適用範囲

最新main60e238e04c1a7fd1bd714d02e928f51f4683de88、PR103 merged・open PRなしを確認し、独立ブランチdocs/ethernet-stp-cost-guide/作業ツリーguide-stpを作成した。README・動的Ethernet仕様/検証仕様・push単位PATCH/履歴規約・全既存記事・Issue91/92/93/95・最新ユーザーコメントを確認した。checkoutにAGENTS.md/.agents/skillsはなく、既存作業ツリー/ブランチは変更していない。全既存記事・nav・入口・23 ZIP・画像を保持する。

正式Releaseはv1.1.4/b45515644dfc65c205216d564d5bf7ef00280622。v1.1.5はtagのみ、Release API404。WSL Ubuntu24.04 LTS x86_64/Rust1.85.0の既存clean dev binaryをmetadataと実ファイルSHA256（0d528291fa6c2c2f643dc3a50707f27ef8866e525d6aec1dcadec97825f8aef8）で照合して再利用した。新規の重い並行ビルド、mainの後続NED修正や最適化buildの性能評価は含めない。

公開dynamic Main.nedとunicast.jsonをbyte同一で使用。公開modelの3 Switchの静的FDBを両条件で共通に空へ整理し、初期unknown unicastとtree適格性を見る。比較する差分はdynamic.linksのL12.cost 10/30だけ。bitrate/伝搬/Switch時間・全up・VID10・全bridge ID等は固定、controlsは空である。sourceのaだけが学習されbは未学習なので、source learningをknown b lookupと混同しない。

独立期待値は[正式版topology仕様](../../../specs/models/Ethernet動的制御詳細機能仕様書.md#topology)から定めた。root s1、lowではs2距離10・s3距離10でL23のdesignatedはID2のs2。highではs2距離20・s3距離10でs3がdesignated、s2のroot portはL23。全12 port roleを固定期待値とする。tagged MAC68byte・wire608bit/占有704bit、1Gbps・伝搬1000ps・Switch2000psから各hopの到着差609000/処理611000ps、b到着1831000/2442000psを独立計算した。

適用はDIR-REQ-0244/0245/0246/0247・DIR-FUNC-0061、既存DIR-TEST-0111の初期三角tree/port選択とDIR-TEST-0112の観測。MAC learn/expireは付随効果も照合するが、仕様や既存検証の完了範囲を変更しない。全体strictを規格適合や全STP組合せ完了とは扱わない。

## CLI・独立ZIP・実Viewer

[measurements.json](measurements.json)は新規出力guide-evidence/stp-freshのrepositoryと隔離ZIPで各2条件×validate/run/view、計12操作exit0。formal commit/clean・実binary/source/lock/6入力hash、manifest全file hash/bytes、全copyのparent・SOF/EOF/release/arrival・VID10/MAC CRC、全12初期roleとlink up、受信status/reason、MAC学習/期限、集計を固定期待値と照合した。隔離ZIPの全simulation objectはrepositoryと同一である。

両条件ともgenerated1/serialized6/forwarded3/received1/filtered2/copy dropped0、filterはstp_discardingとdestination_mismatchが各1。b到着は1831000/2442000ps。complete/partial false/events_exhausted/pending0、最後のeventはMAC失効low6220000/high6831000ps。topology generationは0のままで収束時間の実測ではない。

[viewer-browser.json](viewer-browser.json)はfresh CLI出力・保存測定・現行6入力とmanifestのhashへ結び付けた実Chromium結果。0psの初期role、3Mpsのb到着と拒否枝の詳細、0psへ逆seek一致を検証した。390pxの表は表示枠/内容幅を測り、必要時は水平wheelを実行して右端VIDまで読めることを確認した。公開PNGは600pxの実Policy表2枚で、代替画像は使わない。

## 制約・管理

三角網・VID10・1frame・初期treeだけ。link down/up・非ゼロ収束中の切替・parallel・非連結・VLAN分断・容量・競合の全組合せ、実BPDU/分散STP規格適合は未測定。full Rust/native HTTP suiteも今回ローカルでは再実施していない。管理・執筆・Qiita移植メモは公開記事へ含めない。

Project一覧はINSUFFICIENT_SCOPES/read:projectで拒否された。権限/認証変更や迂回はせず、登録/進行更新は未実施として報告する。Issue closeにつながるDone・マージ/auto-merge・リリース/デプロイ・CodeRabbit操作・外部エージェント依頼は行わない。公開直前コメント/競合とremote head/CIの最終結果はPR本文へ保存する。

## 最終ローカル検証

MkDocs strict、32 HTML・2288ローカルリンク/画像/アンカー、全24 ZIP、第三者notices照合と既存19テスト、適用DIR-REQ-0244～0247と全体traceability strict（231要求・60機能・274 nodes、構造エラー/未完了0）が合格。

[site-browser.json](site-browser.json)は全31 navページの画像読込み・390px幅、コンソール/取得エラー・外部リクエスト0、日本語検索「経路」「三角」で本記事到達を確認した。可視数値表は幅366/内容366pxで横スクロール不要。実Viewerも今回の有効VIDは10だけで、390px時の表示枠/内容360/360px・右端セル375px/枠375pxに収まり、水平wheelは不要で未実行だった（必要時に実操作する分岐はあるが本runでは通っていない）。両条件の600px実Policy PNG、記事desktop/390pxと実測表390pxの実画素を開き、値・role・時刻・可読性を目視した。

公開直前の再確認でもmain60e238e不変、ユーザー追加コメント・open PR/重複作業なし。正確なremote/headとCI最終結果はPR本文へ保存する。
