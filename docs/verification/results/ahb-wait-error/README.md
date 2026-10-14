# AHB応答待ち実習検証記録

文書バージョン：`1.1.2`  
対象GitHubバージョン：`v1.1.4`  
実測ソースcommit：`b45515644dfc65c205216d564d5bf7ef00280622`  
文書ID：`verification-guide-ahb-wait-error`  
文書状態：補助記録（読者サイトの外）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-13` | SHAガードの&&連結と、不一致時にcargoを呼ばない拒否確認を追加 |
| `1.1.1` | `2026-10-13` | 第2push。固定タグビルド・PATHと旧/fresh生成を含む掲載手順を再検証 |
| `1.1.0` | `2026-10-13` | AHB応答待ち3条件・ERROR完了・隔離ZIP・実Viewer・metadata/navとサイトを検証 |

基準mainは `65e862d50626eb3c9e772cdbf6b404de944efc33`。最新Releaseはv1.1.4、v1.1.5はタグのみ。正式v1.1.4のclean CLIをWSL Ubuntu 24.04で使用した。全Issue/PR・既存記事・作業中ブランチを照合し、AHB記事との競合はない。別テーマSoCのPR #78 head `72f431dc953e57922b8c5fa75b4e2674ed5787cb` の成果物を保持して共通入口を統合した。NoCには着手しない。

- CLI：freshへ18操作。公開入力と8入力隔離ZIPの3条件の全simulation・manifest byte/hashを照合。モデル差分はtargets[0].wait_cyclesだけ、INI差分はモデル参照名だけ。validateはnode4/channel0、NEDは6接続。
- 実測：wait0/1/2でa:0は0→20/30/40ps OKAY、b:0は20/30/40→50/70/90ps ERROR。busy50/70/90ps、生成2・完了2・error1・32bit。待ち平均10/15/20、latency平均35/50/65ps、利用率0.5/0.7/0.9。T100、last_event50/70/90、pending_events0、events_exhausted、partial=false、CLI exit_code0、診断null。事前予測は実行結果と独立に照合した。
- Viewer：実Chromium 153.0.8010.12で95psの3条件、a:0/b:0の開始・完了・OKAY/ERROR、counterをassert。5psの待機1・処理中1・完了0と、詳細が最終記録のままであることもassert。PNG3枚の実画素を開いて確認した。経路図はNED配線の正確なトポロジとして扱わない。
- fresh：第1pushの選択先は新規guide-evidence/ahb-final、第2pushは新規guide-evidence/ahb-review-fresh。両browser checkerは明示した9ファイルhashを保存。独立した有効な旧runがあっても省略・不存在・選択Viewer欠落を拒否する6負例に合格。
- サイト：MkDocs strict、20 HTML / 第1push1179・第2push1178ローカル参照、管理メモ除外。404を除く19ページ（17記事＋入口・ライセンス2）の全画像読込・390pxのはみ出しなし・外部要求なし・ページエラーなし、日本語「応答」「ERROR」の記事到達を確認。desktopと390px本文・表の実画素も開いて確認した。
- metadata：実生成器のパーサーで新規2文書の版・タグを検査し、正常→commit併記拒否→復元の負例を確認。対象版はタグ単独行、実測commitは別行。
- 対応：全体strictとDIR-REQ-0190〜0193が構造・未完了0。関連Python59テスト（traceability40・ライセンス19）に合格。既存SoCを含む履歴・画像・ZIPを保持し、全12配布ZIPを検査した。最終文書stageから生成した一覧のfreshness、metadataとnav集合を照合した。原本ライセンス対応はpayload37、lock44（歴史42＋補足2）、crate原本93、source notice10、guide原本8を検査し、法的承認の主張はしない。

[実測と安全なprovenance](measurements.json)、[Viewer検証](viewer-browser.json)、[サイト検証](site-browser.json)を参照する。実行全体の結果とCLI生成Viewerは選択したignored証跡ディレクトリに保存し、公開は安全なprovenance・simulationと実サンプル画像だけに限定する。

第2pushではIssue #80の再現手順2点を修正した。FAQにビルド/PATH手順がないことと、掲載回帰コマンドの旧run未生成を独立確認し、重複Issueがないことを確認して関連付けた。正式タグを新しいclean checkoutへ取得し、commit b455156を確認、Rust 1.85.0でlocked release build（target-dir target）、PATHのCLI選択、AHB validateを実施した。そのCLIで存在しないahb-review-oldとahb-review-freshを順に生成した。各18操作、計36操作の全simulation・隔離ZIP一致と6負例に合格し、freshの実Viewer・サイト報告を更新した。入力ZIP、時刻・応答・指標、既存SoC成果物は変更していない。

追加pushでは、通常のBashで単独test行が失敗しても後続行へ進む問題を同IssueのCLI準備項目として修正した。準備ブロックを&&で連結し、意図的なSHA不一致ではexit1となりcargoが呼ばれないことを確認した。一致時は正式clean checkoutの実cargoでビルド・PATH・AHB validateまで通した。生成一覧の改行はLinux再生成で統一し、手編集していない。

有限2要求、target1、正常アドレスとdecode済ERRORのみ。容量超過・未decode・境界またぎ・複数target・非edge生成・途中打切り・連続負荷・native・実機は未検証。データbyte値、RTL、pipeline/burst/SPLIT/RETRY/lock/pin waveformと実機性能はprofileの保証範囲外。マージ・auto-merge・close・tag/Release・認証変更・デプロイ・外部agent・画像生成は行わない。
