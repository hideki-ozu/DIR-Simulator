# 動的VID登録ガイド検証記録

文書バージョン：`1.1.1`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-vid-registration`
文書状態：CLI・実Viewer・サイト検証合格。Projectは権限不足で未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.1` | `2026-10-16` | 横wheelによる390px表の右端到達を再検証し、スクロール可能な表を表示不能と受け取らせる説明を訂正 |
| `1.1.0` | `2026-10-16` | 初版：VID登録寿命1変数の公開入力、正式版実測・隔離ZIP・実Viewerとサイト検証の証跡を追加 |

## 基準・来歴・適用範囲

最新main 594458ee50aed5c14d4c84d5b68422e26d41d358から独立ブランチdocs/ethernet-vid-registration-guideと作業ツリーguide-vrを作成した。PR100～102はmergedでopen PRなし。README・仕様・既存記事・Issue91/92/93/95・最新コメント、文書push単位PATCHと履歴規約を再確認した。checkoutにAGENTS.md/.agents/skillsは存在しない。既存ブランチ/作業ツリーは変更せず、マージ済みの記事・共通nav・入口・22 ZIP・画像を保持する。

正式Releaseはv1.1.4/source b45515644dfc65c205216d564d5bf7ef00280622。v1.1.5はtagのみでRelease APIは404。正式clean dev binaryをmetadataと実ファイルhashで照合して再利用し、重い並行ビルドは追加しなかった。WSL Ubuntu24.04 LTS x86_64・Rust1.85.0、binary SHA256は0d528291fa6c2c2f643dc3a50707f27ef8866e525d6aec1dcadec97825f8aef8。mainの後続NED修正・最適化buildの性能試験とは区別する。

NEDは公開Ethernet Main.nedとbyte同一。公開VLAN設定由来の動的modelへVID20の静的FDB行と6 portのregistrableを共通追加した。公開registration workloadのgeneratorをMain.a/VID20/生成0,3Mpsへ対応付け、公開vlan_registerを4 port/時刻0へ共通整理した。source/ingress/receiverの寿命は8Mps、egressだけ2M/5Mps。2条件で異なるのはcontrols[3].lifetime_psのみ。6入力ZIPは他記事のファイルに依存しない。

独立期待値は[動的Ethernet仕様membership契約](../../../specs/models/Ethernet動的制御詳細機能仕様書.md#membership)のVID登録・有効VID・known不適格に基づく。static VID10とregistration VID20を区別し、port間へ伝播しない。wire608bit/占有704bit・1Gbps・伝搬1000ps・Switch2000psから最初のarrival609000/転送SOF611000/b1220000、次はarrival3609000/SOF3611000/b4220000psを固定する。shortの既知出口は2Mpsで不適格となり、floodやqueue dropへ読み替えない。

適用はDIR-REQ-0240/0241/0242/0243・DIR-FUNC-0060、既存DIR-TEST-0110のVID登録/期限/適格性とDIR-TEST-0112のViewer観測経路。一部シナリオの証拠を補足し、製品要件・コード・既存検証仕様の完了範囲を変更しない。

## CLI・隔離ZIP・実Viewer

[measurements.json](measurements.json)は明示したfresh出力guide-evidence/registration-freshから生成。repositoryと隔離ZIPの各2条件×validate/run/view、計12操作exit0。formal commit/clean・実binary/source/lock/6入力hash、manifest全file hash/bytes、全copy親子・SOF/EOF/release/arrival・VID20/MAC CRC、受信status/reason/epoch、4登録と4失効の値・予定/実績、policy/集計を独立期待値へ照合した。ZIPの全simulation objectはrepositoryと同一。

shortはserialized3/forwarded1/received1/filtered1（known_egress_ineligible）、longは4/2/2/0、copy droppedは両方0。complete/partial false/events_exhausted/pending0、最終イベント8Mpsを確認した。

[viewer-browser.json](viewer-browser.json)はfresh CLI出力・保存測定・現行6入力とmanifestのhashに結び付けた実Chromium結果。egress expiry-1/expiryと逆seek、4Mpsのregistration3/4件とegressの有効VID、対向bの8Mps登録保持、次frameの受信詳細/カウンター、8Mpsの空登録表と全port静的VID10保持を確認した。公開PNG2枚は実Viewerの600px Policy表であり、代替画像は使わない。

## 制約・管理

単一Switch/VID20/2frame/競合なしのみ。refresh/unregister、登録上限、lookup同時刻全境界、queued copy drop、link/STP変更と複数Switch、実MVRP packetや規格全体適合は対象外。full Rust/native HTTP suiteは今回ローカルで再実施していない。管理・執筆・Qiita移植メモは読者ページへ含めない。

Project一覧のGraphQLはINSUFFICIENT_SCOPES/read:projectで拒否された。認証・権限変更や迂回はせず、登録/進行更新は未実施として報告する。Issueを自動closeするDone、マージ/auto-merge、リリース/デプロイ、CodeRabbit操作や外部エージェント依頼は行わない。公開直前のコメント/競合確認とremote head/CI結果はPR本文へ記録する。

初回390px撮影では有効VID表を横スクロールせずに撮影し、右端列が表示枠の外になったため、比較用画像を600pxで取得した。その時点の「切れる」という説明はスクロール不能と誤解させるため1.1.1で訂正した。390pxでもtable-scrollの横スクロールで右端列を読める。

## 最終ローカル検証

MkDocs strict、31 HTML・2185ローカルリンク/画像/アンカー、全23配布ZIP、第三者notices照合と既存19テスト、適用DIR-REQ-0240～0243/全体traceability strict（231要求・60機能・274 nodes、構造エラー/未完了0）に合格した。

[site-browser.json](site-browser.json)は全30 navページの画像・390px幅、コンソール/取得エラー・外部リクエスト0、日本語検索「登録」「期限」で記事到達を実Chromiumで確認した。実測表の長い理由文字列は本文で説明する形に整理し、390pxで数値表が横スクロール不要の幅に収まることもDOM寸法で確認した。記事desktop/390px、実測表390pxと実Viewer600pxの実画素を開いて可読性と値の一致を目視した。

公開直前の再確認でもmain594458e不変、ユーザー追加コメントとopen PR/重複作業なし。remote head/CI最終結果はPR本文へ保存する。

数値表の追加寸法検査は初回に非表示modalのtableを選んで幅0を記録したため、保存証跡の確認時に検出した。本文role=mainの最後の表へ限定し、幅が0より大きいことも必須として再検証した。最終証跡はその修正後の可視表の値を保存する。

## 390px横スクロールの再確認（第2push）

short/longとも4Mpsで表の表示枠360px・内容402px、overflow-x:autoを確認した。実Chromiumの水平mouse wheel500px入力でscrollLeft0から42へ進み、受信側セルの全文10 tagged, 20 taggedを読めた。セルclientWidth/scrollWidthは150/150で内部文字欠落なし、右端は枠375pxに対し375.109375px（整数scroll丸め0.11px）だった。600pxでは枠/内容570/570pxで横スクロール不要。PNG2枚は第1pushとbyte同一を維持する。

現行main594458eと正式v1.1.4のethernet-app.js blobは8d930480202a111ec59c0573c550c337580d0ba6、ethernet-model.jsは7e60c01fed8550e252c49c7ae4d5c7ad937f0cd1で各々同一。style.cssは正式版の全既存規則に別領域向け規則が追加され、renderPolicyのtable-scroll構造と既存overflow:autoは維持されている。Viewer全体/生成HTML全体の同一性やmain binary再実行は主張しない。

Viewer/390/狭幅/横スクロール/overflowのIssue検索に、この有効VID表のスクロール不能に該当する既知Issueは見つからなかった。Issue95はnative HTTP起動待ちtimeoutで別件。今回の観測は横スクロール可能な正常な表で、現行製品のスクロール不能不具合とは判定せず新Issueは作成しない。

生成short-viewer.html/long-viewer.htmlはローカルguide-evidence/registration-fresh/repository内の非追跡成果物で、リポジトリへ公開保存していない。共有済み証拠はviewer-browser.json・measurements.json・公開PNG2枚と検証スクリプト。追加の公開HTMLアップロードは行わない。
