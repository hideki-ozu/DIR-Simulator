# Ethernet優先キュー実習検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
実測ソースcommit：`b45515644dfc65c205216d564d5bf7ef00280622`  
文書ID：`verification-guide-ethernet-priority`  
文書状態：補助記録（読者サイトの外）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-13` | 論理priority3条件・8入力隔離ZIP・実Ethernet Viewer・FIFO/非プリエンプトを検証 |

基準main65e862d50626eb3c9e772cdbf6b404de944efc33、正式Release v1.1.4、v1.1.5はタグのみ。既存記事・全関連Issue/PR・作業状況・README/仕様/文書規約・AGENTSを確認し、優先キューとの競合はない。PR #78/#79/#83（後者head1b44f12c3d0d7171ae4e21c68160c7dc5c01c8ab）の内容と全履歴を保持して入口を統合する。別担当PR #81と保留修正には触れない。

- CLI：新規priority-old→priority-freshへ順に各18操作、計36操作。各3条件の8入力ZIP隔離再現で全simulation一致、manifest byte/hashを照合。workload generators[1].priorityだけの差分、INI参照名だけを比較。
- 結果：priority0はbusy/fixed/probe、3/7はbusy/probe/fixed。source busy SOF0/EOF576000/release672000psは固定。probe SOFは1344000/672000/672000、b受信2500000/1828000/1828000ps。queue待ち1244000/572000/572000＋serialization1152000＋propagation2000＋processing2000＝受信遅延2400000/1728000/1728000ps。元frame3/copy6/受信3、破棄/未完了0、各使用出力MAC1536/wire1728/occupied2016bit・payload0・利用率0.672、T3000000/last_event2595000、events_exhausted、partial=false、残イベント0、CLI exit0・診断null。
- Viewer：実Chromium153.0.8010.12で500000psの元frame3・待機2・送信中1・EOF/受信0、busy送信中・probe待機、class0/3/7、最終transfer JSONをassert。実選択詳細パネル3枚の画素を開いて確認した。UI状態と実行全体の最終時刻を区別する。
- CLI準備：新しい正式タグcheckoutでSHA照合、Rust1.85.0 locked release build、PATH選択、priority validateを実施した。通常BashのSHA不一致では&&によりexit1・cargo未呼出。新規ビルドのbinary SHA-256は36操作に用いた正式CLIと一致する。
- 負例：独立旧runが有効でも省略・不存在・指定Viewer欠落を拒否する6例、追加field・fixedのpriority・誤probe値・INI終了の比較外変更4例を検査した。metadataは実パーサーで正常→commit併記拒否→復元を照合した。両browserは同じfresh9ファイルhashを保存する。
- サイト：MkDocs strict、22 HTML / 1340参照、管理/Qiitaメモ除外。navと実ブラウザ21ページ＝19記事＋入口/ライセンス2、404込み22 HTMLを区別し、path集合を照合。全画像・390px・検索「優先」「送信」、外部要求/ページエラーなし、desktop/390px本文と表の実画素を確認した。
- 対応：Python59（traceability40・ライセンス19）、全体strictとDIR-REQ-0219〜0223、構造・未完了0。全14 ZIP、既存68資産・ZIPと全履歴を照合して保持した。原本ライセンス対応payload37/lock44（歴史42＋補足2）/crate93/source通知10/guide原本8を照合し、法的承認は主張しない。版・履歴確定後の最終stageからLinuxで生成一覧を再生成し、新規2文書の版・タグをsource/Markdown/HTMLへ照合してfreshnessを確認した。

[実測と安全なprovenance](measurements.json)、[Viewer](viewer-browser.json)、[サイト](site-browser.json)を参照する。全simulationの隔離一致を検査し、公開JSONにも記録・集計を保持する。生のCLI/HTMLはignored証跡ディレクトリへ保存する。

有限3frame、単一送信元・静的FDB・一宛先・空payloadのみ。容量/byte超過、複数送信元、VLAN・flood/multicast、周期/バースト・RX競合・途中停止・連続負荷・native/実機は未検証。PCP wire・TSN/preemption/PAUSE・実機適合/性能は証明しない。deadline=nullで期限判定はしない。マージ・auto-merge・close・Release/tag・デプロイ・認証変更・外部agent・画像生成は行わない。
