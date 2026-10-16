# 源別multicastガイド検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-source-filter`
文書状態：CLI・実Viewer・サイト検証合格。Projectは権限不足で未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-16` | 初版：源別membership mode1変数比較、正式版実測・隔離ZIP・実Viewerと公開サイト検証の証跡を追加 |

## 基準・来歴・適用範囲

最新mainはc67e153cc3bcf1f6ac0303d23023ce8ae4a4f533、正式Releaseはv1.1.4/b45515644dfc65c205216d564d5bf7ef00280622。v1.1.5はannotated tag a1c58a52で、Release APIは404だった。README、源別membership仕様/検証、push単位PATCH改訂・履歴規約、既存記事、Issue91/92/93/95、open PR100/101、最新コメントとレビューを確認した。checkoutにAGENTS.md/.agents/skillsはない。既存ブランチ・作業ツリーを変更せず、PR101 head162160b91679c3b75c93f8c2ea0249ec7128e55fから独立ブランチdocs/ethernet-source-filter-guideと作業ツリーguide-sfを作成した。PR100/101の内容、共通nav・入口・21 ZIPと既存画像を保持する。

正式v1.1.4 clean dev binaryをmetadataと実ファイルSHAで照合して再利用し、新しいCPU負荷の大きなビルドを行わなかった。WSL Ubuntu24.04 LTS x86_64・Rust1.85.0、binary SHA256は0d528291fa6c2c2f643dc3a50707f27ef8866e525d6aec1dcadec97825f8aef8。mainの後続NED修正や最適化ビルドの性能評価とは区別する。

NEDは公開Ethernet Main.nedをbyte同一で使用。公開VLAN設定由来のPR101 modelへ、公開dynamic modelのEndpoint購読を共通追加。静的unicast FDBは保持、静的multicast/routerは空、unknown multicastはflood。公開membership workloadからIPv4 generatorとjoin4だけを取り出し、Net.a/Net.s1をMain.a/Main.sw、portをMain.sw.tx_bへ対応付けた。比較ではcontrols[0].modeだけinclude/excludeへ変更し、sources/key/lifetime/生成時刻等を固定した。新入力は他記事のファイルへ依存しない6ファイルとしてZIPへ同梱する。

期待値は[正式版仕様のmembership契約](../../../specs/models/Ethernet動的制御詳細機能仕様書.md#membership)から独立に定めた。source192.0.2.1は集合内なのでincludeはb候補1、excludeはknown不一致0でfloodしない。期限2Mps後は登録がなくunknown floodのb/c候補2。wire608bit/占有704bit・1Gbps・伝搬1000ps・Switch2000psから、最初のarrival609100/転送SOF611100/b1220100、次frameはarrival3609000/SOF3611000/b,c4220000、最後のrelease4315000psを固定した。

適用はDIR-REQ-0240/0241/0242/0243・DIR-FUNC-0060と既存DIR-TEST-0110の源別不一致/期限、DIR-TEST-0112のViewer観測経路。要件/製品コード/既存検証仕様を変更せず、一部シナリオの新しい証拠として補足する。全体strict成功を規格適合や全組合せの検証完了へ読み替えない。

## CLI・隔離ZIP・Viewer

[measurements.json](measurements.json)は明示した新規出力guide-evidence/source-filter-freshから生成した。repositoryと隔離ZIPで2条件×validate/run/view、計12操作がexit0。formal commit/clean、実binary・source/lock・6入力hash、manifestの全file hash/bytesを照合。全copyのparent・SOF/EOF/release/arrival・MAC CRCと宛先、receptionの時刻/status/reason/epoch、control予定/実績とmembership値、policy・集計を固定期待値と比較した。ZIPの全simulation objectはrepositoryと一致した。

includeはserialized5/forwarded2/received3/filtered0、excludeは4/1/2/1（multicast_filtered）。copy droppedは両方0。set0とexpire2000000、topology generation0、最終complete/partial false/events_exhausted/pending0を確認した。

[viewer-browser.json](viewer-browser.json)は同じfresh CLI出力・保存測定・現行6入力・manifestのhashと結び付けた実Chromium結果。611100psのmembership key/mode/source/期限、expiry-1/expiryと逆seek一致、最初のb配送対multicast_filtered、期限後のb/c受信と最終カウンターを照合した。公開PNG2枚は実Viewerの390px Policy表であり、代替画像は使用しない。

## 制約・管理

単一Switch/VID10/IPv4 group1/source1/2frameの範囲。IPv6、複数源集合、空集合、router/static union、refresh/leave、表容量、STP/link変更、期限とlookupの同時刻等は今回未測定。実IGMP/MLD packetやIP header解析/適合性・実機性能は対象外。full Rust/native HTTP suiteを今回ローカルで再実施したとは主張しない。管理・Qiita移植・執筆メモは公開記事から分離する。

Projectは既知のread:project不足を権限/認証変更で解決しない。登録/進行更新が実施できなければ未実施として報告する。DoneによるIssue close、マージ/auto-merge、リリース/デプロイ、CodeRabbit操作、外部エージェントへの新規依頼や結果待ちは行わない。

## 最終ローカル検証

MkDocs strict、30 HTML・2084ローカルリンク/画像/アンカー、全22 ZIP、第三者notices照合と既存19テスト、適用DIR-REQ-0240～0243と全体traceability strict（231要求・60機能・274 nodes、構造エラー/未完了0）を通過した。

[site-browser.json](site-browser.json)は全29 navページの画像読込み・390px表示、コンソール/取得エラーと外部リクエスト0、日本語検索「集合」「配送」で本記事への到達を実Chromiumで確認した。実ViewerのPolicy PNG2枚、記事desktop/390px・実測表390pxの実画素を開き、値・時刻の一致と可読性、横はみ出しなしを目視した。runtime network::completeとdynamic::select_egressも読み、Switch処理完了で候補集合を選ぶ契約を照合した。

Project一覧GraphQLはINSUFFICIENT_SCOPES/read:projectで拒否された。登録/進行更新は未実施、認証・権限は変更しなかった。公開直前のmain/ユーザーコメント/競合PR再確認とremote head/CI最終結果はPR本文へ記録する。
