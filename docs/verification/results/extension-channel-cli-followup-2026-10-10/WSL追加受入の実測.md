# WSL追加受入の実測

文書バージョン：`1.1.0`  
対象GitHubバージョン：`PR #49 @ 600fd34ba4d24d2dfb6d21a0e586176c798499fa`（測定固定source）  
文書ID：`extension-wsl-acceptance-measured`  
文書状態：対象WSLの限定12回帰・workspace・fmt・clippy実測を確認済み

### 更新履歴

| 文書版 | 日付 | push単位の内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-10 | PR #49第4push。通常VSCodeから指定sourceを実行したWSL追加受入を追補。過去not_run・CI原本を保持 |

## 固定sourceと環境

OMEN35Lの通常VSCode側で2026-10-10 14:33:56–14:38:34 UTCに実行。WSL2 Ubuntu 24.04.4 LTS x86_64、既存rustc/cargo 1.85.0。新規bare repositoryから作成したdetached worktreeのHEADは`600fd34ba4d24d2dfb6d21a0e586176c798499fa`、実行前後のstatusは空。既存checkoutのHEAD・refs・status・worktree一覧は前後一致で、取説の実測は再実行していない。[実行報告](wsl-acceptance-600fd34/execution.json)、[実行器呼出し](wsl-acceptance-600fd34/runner-invocation.json)、[source台帳](wsl-acceptance-600fd34/source-provenance.json)を参照。

## 実行結果

| 対象 | 実測 |
| --- | --- |
| 指定12 regressions | 各`--exact`名を選択し、各1 passed・0 failed・0 ignored、全exit 0 |
| cargo fmt --all -- --check | exit 0 |
| cargo test --locked --workspace | 32群、610 passed・0 failed・0 ignored、exit 0 |
| cargo clippy --locked --workspace --all-targets -- -D warnings | exit 0 |
| runner | exit-code.txtとinvocationとも0、passed_limited |

12件は既存8件とPR #49追加4件であり、workspaceに含まれる試験の別実行である。622種類と数えない。exact名と`test ... ok`・`1 passed; 0 failed; 0 ignored`を全12 stdoutで再確認した。[30ログ](wsl-acceptance-600fd34/public-copy-index.json)に全stdout/stderrを保持する。

受領後、保存49ファイルのSHA256、30ログのbytes/hashとexecution.json、環境preflight、named tests一覧と変更していないrunnerのTESTS、実行9 source input hashesを照合した。[受領検証](wsl-acceptance-600fd34/received-verification.json)。source191ファイルの実行前後hash一致は実行者の保存記録に基づく。このPCでbyteを照合できた13 source files（runner・対象Rust/試験など）は公開600fd34のGit treeの各blobとも一致する。191全ファイルを独立再取得したとは主張しない。

## 原本・現在の受入・制約

原本のexecution.json、全ログ、source台帳、保存hash一覧、既存作業の前後記録は受領フォルダを変更せず保持した。公開コピーでは個人の絶対パスのみをplaceholderに置換し、[原本hashと公開copy hash](wsl-acceptance-600fd34/public-copy-index.json)を区別する。公開execution.jsonのstream hashは原本を指し、公開copyのbytesとは一致しない場合がある。

過去のlocal-execution-not-run.json・verification.jsonと第1fmt失敗/既存CIログは当時の履歴としてbyte保持する。現在の対象WSL追加実測待ちは今回の証跡で解消する。新しいpushは文書・証跡・生成一覧だけで、測定sourceのRust、runner、workflowを変更しない。

全channel capability、全profile、全仕様・AC0008適合、finish/panic全matrix、ライセンス採用・作者承認を示さない。merge・Release・deploy・Issue close・権限設定変更は行わない。
