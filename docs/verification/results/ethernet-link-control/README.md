# link復旧ガイド検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-link-control`
文書状態：CLI・実Viewer・サイト検証合格。Project操作は権限不足で未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-16` | 初版：link復旧1ps境界の公開入力・正式版CLI・隔離ZIP・実Viewerとサイトの証跡を追加 |

## 基準と適用範囲

mainはc67e153cc3bcf1f6ac0303d23023ce8ae4a4f533。正式Releaseはv1.1.4（b45515644dfc65c205216d564d5bf7ef00280622）、v1.1.5はタグのみ。README・動的Ethernet仕様/検証仕様・文書push改訂/履歴規約・既存記事・open Issue/PR・PR100コメントを確認した。AGENTS.mdとcheckoutの.agents/skillsは存在しない。既存作業ツリーは変更せず、PR100 head5a7bb259から独立ブランチdocs/ethernet-link-control-guideを作成し、PR100の記事・nav・入口・20 ZIPを維持した。初回checkoutはWindowsの長いファイル名で失敗し、短い独立パスguide-lcで作成し直した。

正式v1.1.4のclean dev binaryをSHAとmetadataで照合して再利用し、CPU負荷の大きなビルドを追加しなかった。WSL Ubuntu24.04・Rust1.85.0。binary SHA256は0d528291fa6c2c2f643dc3a50707f27ef8866e525d6aec1dcadec97825f8aef8。最適化ビルドの性能評価やmainの後続NED修正を正式版の実測とは扱わない。

NEDは公開Ethernet Main.nedをbyte同一で使用。公開VLAN all設定をschema4へ共通正規化し、1 bridge/3 linksのdynamic設定を追加した。負荷は公開dynamic topologyのNet.aをMain.aへ対応付け、downを500000ps、upを609000psへ共通整理。比較ではup.at_psだけ609001psへ変える。静的FDBと2 frame生成時刻0/2000000psは共通である。

独立期待値は[動的Ethernet仕様のtopology契約](../../../specs/models/Ethernet動的制御詳細機能仕様書.md#topology)。MAC68byte・wire608bit・占有704bit、1Gbps・伝搬1000ps・Switch処理2000psからSOF0/EOF608000/arrival609000、転送SOF611000/arrival1220000を事前固定する。phase0復旧が同時刻phase1 arrivalより先に適用される。次frameは2Mps加算、最後のrelease3315000ps。

適用はDIR-REQ-0244/0245/0246/0247・DIR-FUNC-0061、既存DIR-TEST-0111のSOF済み継続/入口判定とDIR-TEST-0112のViewer観測を補足する。要件・製品コード・既存検証仕様の完了範囲を変更しない。全体strict成功をSTP全組合せや規格適合の証明へ読み替えない。

## 実測・再現・実画面

[measurements.json](measurements.json)は新規出力guide-evidence/link-fresh-verifiedから生成。repositoryと隔離ZIPの各2条件×validate/run/view、計12操作がexit0。6入力hash・formal commit/clean・実binary hash・source/lock hash、manifestの全file hash/bytesを確認した。全copyのparent・SOF/EOF/release/arrival・CRC、受信status/reason/epoch、control予定/実績、policy世代、集計を独立期待値へ照合した。隔離ZIPの全simulation objectはrepositoryと同一。

on-timeは4 serialized/2 forwarded/2 received/0 filtered。lateは3/1/1/1で唯一のfilter理由はlink_down。copy droppedは両方0、次のb到着は両方3220000ps。両runともcomplete、partial false、events_exhausted、pending0。

[viewer-browser.json](viewer-browser.json)は同じfresh出力・現行入力・保存測定・manifestのhashと結び付けた実Chromium検証。down-1/down、SOF済みcopy、609000psのup/down差、609001→609000の逆seek一致、1300000psの受信詳細、4000000psの次frame配送を確認した。Policyの390px実PNG2枚を取得した。

初回shell実行はPowerShell変数展開で失敗し固定.shへ移した。次は出力親ディレクトリ不足でrun exit4となり、親を作成して再実行した。初回checkerはfilteredのready_psをnullと仮定して失敗したが、実契約では拒否時刻609000でplanned_ready_psがnullである。独立のreception判定・時刻期待を維持してそのfield検査を訂正し、別の新規出力で12操作すべてを再実行した。製品不具合として隠さずchecker/実行準備の失敗と区別する。新規venvの通常pipはDNSに失敗したため承認済みネットワーク実行で既定requirementsを取得した。

## 制約・管理

一つのEndpoint link・VID10・2 frame・convergence0の境界のみ。queue drop、down中の新規source、複数Switch/STP非ゼロ収束、全停止境界、PHYの実bit破損/同期やTCP再送は対象外。full Rust/native HTTP suiteは今回ローカルで再実施していない。執筆・Qiita移植・管理メモを公開記事に混ぜない。

Projectは既知のread:project不足。認証/権限変更や迂回を行わず、登録・進行更新は未実施として報告する。Issueの自動closeにつながるDone操作、マージ、auto-merge、デプロイ、CodeRabbit操作、外部エージェントへの新規依頼や結果待ちは行わない。

MkDocs strictの初回はdocs_dir外への相対仕様リンク2件で失敗した。正式v1.1.4のGitHub blob参照へ訂正し、strictとリンク検査を再実行した。生成サイトを再build中に開始した初回ブラウザー巡回もh1件数検査で失敗したため、build完了後の静的成果物で独立にやり直す。失敗を成功へ読み替えない。

## 最終ローカル検証

MkDocs strict、29 HTML・1985ローカルリンク/画像/アンカー、全21配布ZIP、第三者notices照合と既存19テスト、全体traceability strict（231要求・60機能・274 nodes、構造エラー/未完了0）を通過した。適用DIR-REQ-0244～0247もstrictで確認した。

[site-browser.json](site-browser.json)は実Chromiumで全28 navページの画像読込み・390px表示・コンソール・外部リクエストを検査し、日本語検索「復旧」「受信」で本記事への到達を確認した。エラー・取得失敗・外部リクエスト0。実Viewer Policy PNG2枚、記事desktop/390px・実測表390pxの実画素を開いて、値と時刻の一致、可読性と横はみ出しなしを目視した。

Project一覧のGraphQL読み取りも今回再試行しINSUFFICIENT_SCOPES/read:projectで拒否された。登録・進行更新は未実施。PR100はDraft/open・head5a7bb259のままでレビュー/行コメントなし、mainはc67e153。公開直前の再確認でもmain c67e153・PR100 head5a7bb259に変化なし、ユーザー追加コメントと他のopen PRなし。
