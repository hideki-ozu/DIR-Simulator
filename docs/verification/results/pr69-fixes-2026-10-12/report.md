# PR #69 不具合修正検証

文書バージョン：`1.1.2`
対象GitHubバージョン：`main @ dbcbcc5`
文書ID：`verify-pr69-fixes-2026-10-12`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-12` | mainへのbase移行後に依存ブランチ向けCI条件を撤去した事実を明示し、初回構成との区別を補正 |
| `1.1.1` | `2026-10-12` | Viewer回収snapshotと遅延応答の競合を直列化し、旧422のbarrier再現と修正後成功を記録 |
| `1.1.0` | `2026-10-12` | Issue #74：予定GitHub版1.1系へ文書MAJOR.MINORを訂正しPATCH0から開始。公開済み1.0.0履歴を保持。ユーザーのPR69マージ後のmainと検証対象を区別 |
| `1.0.0` | `2026-10-12` | Issue #70〜72 の境界・参照・耐久化の修正と実行検証、既存CI修正との責務分離を記録 |

## 依存・所有

独立ブランチ `pr69-runtime-viewer-fixes`、基準head `c23855ab3f74e71de197f1e457676a6f48a438c0`。PR #69 の作者ブランチにはpushしていない。作者が更新したrestart fixtureとlarge response試験を保持。元CIのrestart競合は作者の正しいflock同期で解決し、runtime Lifetimeは書き換えない。別の重複Issueは作成しない。

Issue [#70](https://github.com/hideki-ozu/DIR-Simulator/issues/70) は期限キャッシュ、[#71](https://github.com/hideki-ozu/DIR-Simulator/issues/71) はHTTP終端ledgerと公開pending、[#72](https://github.com/hideki-ozu/DIR-Simulator/issues/72) は所有HTMLの寿命を担当する。追加機能・結果schema・モデル指標の変更ではない。

## 実行検証

PR #69 はユーザーにより main `dbcbcc549e737c6dd68d544bf61f53ce5811bba7` へマージされたため、修正PR #73 のbaseをmainへ変更した。通常mergeで追従し、履歴書換えは行わない。文書版の訂正は [Issue #74](https://github.com/hideki-ozu/DIR-Simulator/issues/74) で追跡する。元1.0.0は誤った系列で公開した履歴として保持する。

HTTPとHTMLの追加回帰は、隔離treeで新しい復旧・回収だけを無効化した旧挙動で、それぞれ503対200、最終参照後のファイル残存によりFAILした。修正版の同じ試験はPASS。test hook/所有参照harnessを加えた再現であり、旧headの無改造実行とは区別する。

- `ned_editor_extensions_ui.test.cjs` の期限境界試験は修正前 FAIL（期限到達後もanchorあり）、修正後 PASS。既存NED UI/recoveryと合わせ81試験 PASS。
- 実Chromiumの `ned_editor_viewer_expiry_browser.test.cjs` は公開済みc23855aのinput/viewを読み同じ時計試験で FAIL、修正版で PASS。1時間を実時間待たず、期限直前/到達、同run再描画、pollなしの古いanchorクリック、再準備2回、read-only維持、別タブ遷移防止を確認。準備応答はこのUI境界試験では制御したcallbackを使う。
- WSL Ubuntu 24.04 の既存Rust toolchainでHTTP ledgerの新回帰1件 PASS。最初の終端エラーを5回再送し、別commandの成功・2件の一意なrunを確認し、旧command再送でも件数不変。
- HTML回収の追加2件 PASS。20世代で最終応答参照までファイルを保持し、run削除を503で防いだ後、ファイル・inventoryを回収。中断で既知ファイルだけ既に消えた場合の修復、未確定予約・差替えファイルの保全を確認。
- Windows checkoutをWSLから使う全libraryと実NEDブラウザの初回は改行依存試験で失敗。INIのCRLFを既存fixtureが再びCRLFへ変換しbare CRとなる例と、INIの期待末尾LF/実際CRLFの差を確認。試験期待値を変更せず、`git -c core.autocrlf=false archive` の隔離正本へ修正を重ねて再実行する。リポジトリ設定・取得済み証跡の元bytesは変えない。
- LF正本と同じ正本からビルドしたCLIによる実NED後段ブラウザ回帰は PASS。新規入力/COW/rename/Undo/Redo/export/CLI validate-run、公開・過去・partial run、Viewer step/revoke/deleteまで既存gateを実行した。機密URLをredactする既存evidence出力を利用し、秘密値は本報告へ保存しない。

最終library/明示実ワーカー・loopback/文書・remote exact-head CIはPR本文へ実行結果を記録する。初回の依存base向けpull_request条件は、ユーザーのPR69マージ後のmainへのbase移行時に撤去した。現在のbranch条件は元のmainのみである。チェック名・permissions・main push条件は保持し、NED UI回帰と明示実ワーカー・loopback試験を既存workflowへ加える。マージ、auto-merge、release/tag/deploy、Issue/PRクローズは実行しない。

最終LF libraryは430 PASS / 5 ignored。5件は同じ最終ビルドを指定して明示実行し、5 PASS / 0 ignored。初回force-kill試験はSIGTERM(15)を観測しSIGKILL(9)期待で失敗。SIGSTOP送信が停止到達を保証しないため、pidfd waitid(STOPPED|NOWAIT|NOHANG)でkernelの停止を確認してからcancelする試験同期へ変更した。SIGKILL期待・runtime停止処理・作者のrestart同期は維持。文書40試験・strict（231要件/60機能/274 nodes/エラー0）、MkDocs strict・16 HTML/875リンクはPASS。

Viewerの旧世代回収はinventory snapshotからbatch完了まで同じmutexを保持する。遅延した最後の応答による二度目の回収は、inventoryとファイルが両方不存在の場合だけ削除なしで成功する。同名の未知ファイルがあれば拒否する。旧公開cleanupへbarrierだけを加えた同じ試験はUnknown Viewer allocation/422で失敗し、修正版は成功。全target clippyも成功。Issue #75で追跡しクローズしない。
