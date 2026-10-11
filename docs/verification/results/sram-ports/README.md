# SRAMポート数ガイド検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
文書ID：`verification-guide-sram-ports`  
文書状態：実施記録。Project登録は親タスクへの引継ぎ対象

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-11` | 初回push。正式タグCLI・隔離ZIP18操作、実Viewer、サイト・検索・適用traceabilityの検証記録 |

## 基準と選定

取得mainは`bbd446deaaa6fd743a17b6cd3a75207ef2083cd4`。最新正式Releaseはv1.1.4、`b45515644dfc65c205216d564d5bf7ef00280622`。全Issue/PR・remoteブランチ・既存記事を確認し、SRAMポート数を選定。同テーマの進行中作業は見つからなかった。PR58とIssue52/53のViewer開発は紹介・編集対象に含めない。PR36の入口、既存画像、4種のZIP、過去の分岐履歴を保持する。リポジトリ内AGENTS.mdと関連skillsの所在を確認し、適用するローカル追加指示はなかった。外部エージェントへ依頼していない。

## 実施結果

[measurements.json](measurements.json)はWSL Ubuntu 24.04の正式タグ・cleanビルド（Rust1.85.0）の実測。埋込Git SHA、binary/build-source/Cargo.lock/input SHA-256を保存した。`verify_guide_sram.py`でvalidate/run/viewを3条件×2入力元、計18操作実行。モデル設定の差が`sram[0].ports`だけであること、生成・開始・完了時刻、3件の返却バイト、最終メモリ、offered/completed/rejected、全manifestハッシュをassertした。ZIP8入力を隔離展開し、全simulationオブジェクトが一致した。

実ViewerはChromium149.0.7827.55で150 psへ移動し、c:0を選択。カウンタ・要求一覧・詳細schemaをassertして3画像を取得。[viewer-browser.json](viewer-browser.json)と公開画像の実画素を開き、1ポートで待機1/処理中1/完了1、2ポートで0/1/2、3ポートで0/0/3を確認した。最終JSONと現在状態の違いも本文へ明記した。

MkDocs1.6.1 strictで404を含む13 HTMLを生成。`check_guide_links.py`で661のローカルリンク・画像・アンカーと執筆メモの非公開を確認。全5 ZIPと正本の一致を確認。[site-browser.json](site-browser.json)のpages配列12件（404除外、読者向け10ページ＋入口＋ライセンス）を実ブラウザで閲覧。全画像読込、390pxの文書横幅、page error/HTTP failure/外部通信ゼロを確認した。日本語検索「ポート」「読み出し」は新記事へ到達。desktop、390px冒頭・実測表、検索画像の実画素を開いて確認した。

既存実装の解説で要件・機能・trace nodeを追加しない。SRAM仕様のDIR-REQ-0202/0203/0204/0205、DIR-FUNC-0048を対象とする。全体strictと対象要件strict、traceability回帰40テスト、生成一覧の鮮度を検証する。最終CIとremote/headの確認結果はDraft PR本文へ記録する。

## 制約・障害

- 実習は抽象SRAMの同時読出しのみ。実機性能、CPU/cache/AXI、書込み競合、容量不足や終了境界の受入を証明しない。
- 初回checkerはCLI JSONの終端キーを`finish_ps`と誤認して失敗。実際の`simulation.end_ps`へ修正し、新出力先で18操作を全て再実行した。失敗を合格へ流用していない。
- Project参照は既存ghトークンに`read:project`がなく拒否された。認証・権限変更はせず、Draft PR URLを親タスクに渡して既存「DIR取説整備」Projectへの登録・進行状態設定を引き継ぐ。未実施を成功扱いしない。
- マージ、auto-merge変更、Issue/PR close、force push、履歴書換え、デプロイ、外部レビュー依頼は実施しない。
