# メールボックス容量ガイド検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
文書ID：`verification-guide-mailbox-capacity`  
文書状態：実施記録。Project登録は親タスクへ引継ぎ

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-11` | 初回push。正式CLI・隔離ZIP・fresh証跡回帰・実Viewer・サイト・適用traceabilityの検証を保存 |

## 基準と独立テーマ

取得mainは`f2ac9189b48241fb4496091cd6ab27b0d6bedeac`。正式Release/tagはv1.1.4、`b45515644dfc65c205216d564d5bf7ef00280622`を再確認した。全Issue/PR・remoteブランチ・既存ガイド・アプリの進行中一覧を確認し、open Issue/PRは0件。Gateway処理遅延、SRAMポート数、既存CAN/Ethernet記事と異なる「メールボックスの容量と満杯応答」を選択。同テーマの進行作業は見つからなかった。

README、メモリ・IPC詳細仕様、文書運用規約、ガイド統合手順、AGENTS.md/関連skillsの所在を確認。リポジトリ内にAGENTS.md/.agentsはなく、祖先に適用する追加AGENTS指示はない。ユーザーAGENTS.mdは空。外部エージェント・CodeRabbitへ依頼していない。PR58/60はmainへ統合済みだが、この記事は正式タグViewerのみを使う。PR36の入口、既存5種類のZIP、既存画像、過去の更新履歴を保持した。

## 実施結果

実測は既に確定した正式タグのcleanビルドを読み取り利用した。WSL Ubuntu 24.04、Rust1.85.0。埋込git_commit/dirty、binary/build-source/Cargo.lock/input SHA-256を[measurements.json](measurements.json)へ保存。メタデータの個人パスやCPU情報は公開コピーに含めない。

最終fresh証跡`guide-evidence/mailbox-final`で3条件のvalidate/run/viewと隔離ZIP展開先の同じ9操作、計18操作を実施（準備時の別fresh出力でも18操作実施）。8入力とZIPのバイト一致、1変数mailboxes[0].capacity、6要求の発生・開始・完了・reason・message_id・バイト内容、FIFO、通知時刻、最終メールボックス、指標、全manifest hashをassert。隔離ZIP実行とリポジトリ実行の全simulationオブジェクトが一致した。

各条件の成功send/receiveと実通知は容量に等しい1/2/3件。full/emptyは2/1/0件。全条件でoffered=completed=6、rejected=0、終端1000 ps、最終イベント900 ps、partial=false。full/emptyを受付拒否や成功として数えない。記事の成功件数は要求行から算出した値で、存在しないmetric名を作っていない。

両ブラウザcheckerはfresh出力を明示引数で受け、選択したViewer/results/manifestの9ファイルのhashを記録。[viewer-browser.json](viewer-browser.json)は実Chromium149.0.7827.55で950 psへ移動してsend_c:0を選択し、6行・完了6/拒否0・詳細full/full/ok・資源を確認した結果。公開3画像の実画素も開き、byte数4/8/12と通知1/2/3、最後の資源が空であることを目視した。

有効な旧出力mailbox-verifiedが残った状態で、両checkerの引数省略・存在しない指定先・fresh側Viewer欠落の6負例を実行。終了コード1と対応するエラー原因をassertし、旧結果が代替利用されないことを確認。退避ファイルはfinallyで復元した。その後同じfresh証跡でViewer・サイトを確認した。

MkDocs1.6.1 strictは404を含む14 HTMLを生成。729ローカルリンク/画像/アンカーと執筆メモ非公開、全6 ZIPの正本一致を確認。[site-browser.json](site-browser.json)のpages配列13件は404を除く読者向け11ページ＋入口＋ライセンス。全ページで画像読込、390pxの文書横幅超過なし、外部通信/page error/HTTP failureゼロを確認。日本語検索「容量」「満杯」は新記事を発見した。desktop、390px冒頭・実測表、検索のPNGを実画素で確認した。

traceability40回帰テスト、全体strict、DIR-REQ-0214/0215/0216/0217の適用strictを確認。231要件・60機能・274node、構造エラー/未完了0。既存DIR-FUNC-0051とspec-memory-ipc#mailboxを解説し、新規要件・機能・trace nodeは追加しない。第三者通知9回帰テストと元バイト対応も合格。push単位文書版を確定してステージした後、生成一覧を正規スクリプトで更新し、鮮度を確認する。exact head CI終端とremoteはPR本文へ記録する。

## 障害と制約

- 準備中に生成器IDのハイフンが識別子規則違反、未作成の出力親ディレクトリがoutput_write_failedになった。IDをunderscoreへ変更し、出力親の明示作成を本文へ入れ、最終freshで全条件を実行し直した。loaderのexport名を置換で壊した初回ブラウザ試行も修正して再実行し、失敗を成功の証跡に流用していない。
- Project参照は既存ghトークンのread:project不足で拒否された。認証・権限は変更せず、PR作成後に親タスクへ既存「DIR取説整備」への登録・進行状態設定を引き継ぐ。未実施を成功扱いしない。
- 実機IPC、OS/CPU割込、再送、権限拒否、queue_full、終端未通知の受入はこの比較の範囲外。正式タグの抽象単一server/FIFOモデルだけを説明する。
- マージ、auto-merge変更、Issue/PR close、force push、履歴書換え、デプロイ、Release/tag作成は実施しない。
