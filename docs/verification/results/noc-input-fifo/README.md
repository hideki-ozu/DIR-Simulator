# NoC入力FIFO実習検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
実測ソースcommit：`b45515644dfc65c205216d564d5bf7ef00280622`  
文書ID：`verification-guide-noc-input-fifo`  
文書状態：補助記録（読者サイトの外）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-13` | NoC入力FIFO3条件・隔離ZIP・35ps実Viewer・metadata/navとサイトを検証 |

基準mainは65e862d50626eb3c9e772cdbf6b404de944efc33。正式Releaseはv1.1.4、v1.1.5はタグのみ。既存記事・全Issue/PR・作業中ブランチ・文書規約とAGENTSを再確認し、NoCとの競合はない。別テーマPR #78 / #79（後者head43ebdbcfabb5d8f1444f1d7268e1618b8d8ea296）の成果物・全履歴を保持して入口を統合した。

元fixtureからsource_capacity1→4、aの最終生成20→30ps、終了65→90psを全条件共通に変更した。その共通条件からinput_capacityだけを1/2/3へ変更する。元fixtureから容量だけ変えた比較とは扱わない。

- CLI：noc-oldとnoc-freshを新規出力として順に生成。各18操作・計36操作、8入力ZIP隔離再現で全simulation一致。manifest byte/hash、1変数差分、5transaction/9transfer、各grant・hop・応答・配送量を検査。
- 実測：aの最初のgrantは容量1で0/40/50/60、容量2で0/10/40/50、容量3で0/10/20/40ps。a最終配送は全条件50/60/70/80ps、bは0→40ps local配送。a待ち平均22.5/10/2.5・latency50、全体待ち18/8/2・latency48ps、生成/完了5、256bit、error/drop0。r00 east占有40・r10 local占有80ps、T90、last_event80、pending_events0、events_exhausted、partial=false、CLI exit0・診断null。事前予測と独立実測は一致した。
- Viewer：実Chromiumで3条件の35ps、生成5・待機3/2/1・処理中2/3/4・完了0、a:1の一覧状態と最終JSON、east/localの2transferをassert。処理中にはhop間FIFO待ちを含み、詳細は選択時刻の観測値ではない。NoCにメモリsnapshotを仮定しない。PNG3枚の実画素を開いて確認した。

- CLI準備：正式タグを新しいclean checkoutへ取得し、Rust1.85.0でlocked release build・PATH選択・NoC validateを確認した。&&で連結したSHA不一致では通常のBashでもexit1となりcargo未呼出。新規ビルドのSHA-256は測定に用いた正式CLIと一致する。
- 負例：旧/freshを明示し、引数省略・不存在・fresh Viewer欠落の6例で旧成功出力を代替利用しないことを検査。別field・source容量・誤った比較容量・INI終了時刻の4変異を、CLI呼出前に拒否した。metadataは正常→commit併記拒否→復元を実パーサーで確認した。
- サイト：strict、21 HTML / 1258参照、管理メモ除外。nav/実ブラウザは20ページ＝18記事＋入口/ライセンス2、404込み21 HTMLを区別する。390px、全画像読込、日本語「容量」「FIFO」の記事到達、ページエラー/外部要求なしを確認した。desktop/390px本文・表の実画素も開いて確認した。経路表記に空白を入れて折り返し可能にし、共通CSSを変えず横はみ出しを解消した。
- 対応：全体strictとDIR-REQ-0194〜0197は構造・未完了0。関連Python59（traceability40・ライセンス19）に合格。全13 ZIPと原本ライセンス対応payload37/lock44（歴史42＋補足2）/crate原本93/source通知10/guide原本8を照合し、法的承認は主張しない。最終文書stageから生成一覧をLinuxで再生成し、freshnessと新規文書の版・タグをMarkdown/HTMLの両一覧に照合した。

進行中PR #81は別作業の契約・診断・仕様変更であり、このガイド・入力・画像との競合はない。今回の実測と契約参照は正式v1.1.4に固定し、未マージPRの変更を実装済みとして紹介しない。

[実測と安全なprovenance](measurements.json)、[Viewer検証](viewer-browser.json)、[サイト検証](site-browser.json)を公開する。生のCLI出力と撮影用HTMLはignored証跡ディレクトリへ保存し、個人パスを公開しない。

有限5packet、2列1行、常時受理終点、一つの競合出力のみ。source満杯drop、別topology・連続負荷・非edge生成・途中停止・deadlock診断、native・実機は未検証。有限slot/store-and-forwardの抽象評価で、wormhole/flit・VC・adaptive・RTL/波形/規格適合や実機性能の受入証明ではない。マージ・auto-merge・close・Release/tag・デプロイ・認証変更・外部agent・画像生成は行わない。
