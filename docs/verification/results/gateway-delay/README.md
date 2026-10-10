# Gateway処理遅延ガイド検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
文書ID：`verification-guide-gateway-delay`  
文書状態：実施記録。Project登録は環境・権限で未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-11` | 初回push。正式タグCLIと独立ZIPの18操作、実Viewer、サイト表示・検索・適用traceability検査を保存 |

## 基準と独立テーマ

- 取得main: `1a0008077a65b2403b20ef8375ce860d4fa29772`。push前のremote再確認でも同じ。
- 最新正式Release: v1.1.4、`b45515644dfc65c205216d564d5bf7ef00280622`。
- WindowsとWSLのAGENTS.md・関連skillsの所在を確認。リポジトリ内のAGENTS.md/.agentsは存在しない。Windows側のユーザーAGENTS.mdは空。外部エージェント依頼は行っていない。
- 全Issue/PR、全remoteブランチとアプリ内進行状況を確認。取得時にopen PRなし。既存記事のCAN入門、Viewer、調停、受信フィルタ、受信遅延、CAN FD、Ethernet期限とは独立した「複数CANバスのGateway処理遅延」を選択。同テーマの進行中作業を検出していない。
- Issue52/53は未採用提案として区別。PR36のCC/FD入口、全既存nav、画像、3種の配布ZIPと分岐履歴を保持。

## 実施済み

正式タグを専用のWSL checkoutでビルド。Ubuntu 24.04 LTS x86_64、rustc 1.85.0。最終結果の埋込出自はタグSHA、git_dirty=false。binary/build-source/Cargo.lock/inputのSHA-256は[measurements.json](measurements.json)へ保存。

`scripts/verify_guide_gateway.py`で3条件それぞれvalidate/run/viewを実行し、さらに隔離したZIP展開先で同じ9操作を実行。計18操作、9入力、1変数差分、元SOF/EOF、コピー生成・SOF/EOF、受信完了、RX保持解放、親子ID、24データファイルのmanifest hashを確認。ZIP実行とリポジトリ実行のsimulation全体（全model_records・records・summary等）が一致。

Viewerは生成済みHTMLを実Chromium 149.0.7827.55で開き、400µsへ移動しsource:0を選択。B/Cの現在状態を直接assertし、[viewer-browser.json](viewer-browser.json)と公開assetsの3画像に記録。画像は別途実画素を開いて、0µsでB送信中/C送信成功、100µsでB/C送信中、300µsでGW RX1/4・コピー生成前を確認した。架空・合成のViewer画像は使用していない。

MkDocs 1.6.1 strict、ローカルリンク/画像/アンカー595件、公開asset・執筆メモ除外検査、全4ZIPの正本一致、第三者通知の元バイト対応と9件の回帰テストを確認。[site-browser.json](site-browser.json)は全12 HTMLページ（読者向け9ページと付属ページ）の画像読込、390pxで文書横幅超過なし、外部通信・page error・HTTP failureなし、新記事への日本語検索「中継」「遅延」を記録。desktop・390px本文・実測表・検索のPNGも実画素を確認。

既存仕様・実装を解説する追加記事で、要件・機能・trace nodeは追加しない。GW仕様のconfiguration/forwarding/payload-resultsを参照し、DIR-REQ-0123/0126/0128に対する`check_traceability.py --strict --requirement`を実施。各検査は231要件・60機能・274 node、構造エラー/未完了0。生成一覧・階層HTMLを規約のpush単位改訂確定後に再生成し、`generate_traceability.py --check`を確認。

## 障害・制約

- Project「DIR取説整備」への登録・進行状態設定は未実施。既存gh認証にread:project scopeがなくProject参照が拒否された。ブラウザ実行基盤も必要なWindows platform directoryを見つけられず起動不能。認証・権限設定は変更していない。Done・Issue closeも実施していない。
- 日本語単語検索「中継」「遅延」は新記事を発見できる。連結語「処理遅延」は検索結果が出ず、30秒待ちが失敗した。検索tokenizer全体の変更は今回の範囲外。
- 390pxの比較表は表内の横スクロールで全列を読む。文書全体の横幅超過とは区別する。
- 実習は固定経路・1元要求・2コピー・競合なし。TX満杯、RX破棄、hop超過、終了境界、規格全体適合を今回の検証合格へ含めない。
- 初回Windows cloneで長いパスcheckout失敗、Windows既定文字コードの検査失敗、改行差によるZIP/通知テスト失敗、Windows worktree参照によるunknown/dirtyビルド出自を観測した。最終CLIは独立WSL checkoutでcleanな正式タグを使用し、UTF-8・元Gitバイトで最終検査をやり直した。途中の失敗結果を最終成功の証拠へ流用していない。

Draft PRのremote/head CI終端確認はPR本文に記録する。マージ、自動マージ有効化、デプロイ、Issue/PR close、force push、履歴書換えは行っていない。
