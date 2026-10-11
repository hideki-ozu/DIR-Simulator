# 公開文書整合性の監査記録（2026-10-11）

文書バージョン：`1.1.2`
対象GitHubバージョン：`v1.1.4`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-11` | 追加pushとしてPATCHを1増加し、5文書の対象版整合と初回採番誤りの訂正を追記。push済み履歴は保持 |
| `1.1.1` | `2026-10-11` | 公開文書整合性の監査記録を確定し、依頼に従いPATCHを一つ上げ、対象タグをv1.1.4へ統一 |

文書ID：`document-consistency-2026-10-11`

監査で検出した現在状態・API・schema・モデル動作・画面操作説明の不一致を修正した。仕様、設計、検証仕様、ツール説明、ガイド、要件・機能仕様と生成一覧へ反映した。製品ソースの変更はない。[機械可読の監査記録](document-consistency-2026-10-11.json)に対象文書のhash、リンク検査、試験結果と全サンプルの個別結果を保存する。

## 対象と基準

開始時のcheckoutは`main @ 0b7b23d8e23fcb1a1491cd13cd42aa5b96a9ab1c`で、trackedファイルの未commit変更はなかった。Git管理下のREADMEとdocsを列挙し、Markdown 102件、HTML 10件、PlantUML 11件、SVG 11件を対象にした。仕様21件、設計18件、検証仕様18件を含む。関連する画像・入力・ライセンス原文・過去のソースsnapshot等を含めたREADME/docsのtrackedパスは3,202件、そのうち検証結果配下は2,219件である。この監査記録自体は開始時の件数に含まない。

現在形の公開・実装状態は、公開済み[v1.1.4](https://github.com/hideki-ozu/DIR-Simulator/releases/tag/v1.1.4)、現行Registryとソース、実在する検証記録を基準にした。API・入力・出力の契約はRust宣言、serde設定、CLIの実測、サンプルの実行結果と照合した。追加モデルの抽象化範囲、未確認の受け入れ条件、native基準機の残件は保持した。

過去の実施記録・fixture・ソースsnapshotは記録時点の証跡として扱い、現在の実装状態に書き換えていない。公開版の履歴行も保持した。既存文書のPATCHと確定履歴は[push時の版確定規則](../../ドキュメント作成・運用規約.md#document-version-at-push)に従い、今回のローカル編集では増やしていない。

## 修正した主な不一致

| 分野 | 修正内容 | 根拠・主な文書 |
| --- | --- | --- |
| 公開状態・提供範囲 | 公開済みv1.1.4を「本PR・公開予定」とする現在形を更新。提供済みprofileを一律に開発中・将来実装とする説明と、設計文書数17件の記載を修正 | [README](../../../README.md)、[要件](../../要件定義書.md)、[機能仕様](../../機能仕様書.md)、[拡張計画](../../将来拡張計画.md) |
| 実施状態 | 一律の「全製品ケース未実施」「測定未実施」を、実施記録と残条件への参照へ更新。2026-10-04等の過去の試験表は記録日時点の状態を保持 | [入力・設定検証](../cases/入力・設定検証仕様書.md)、[受け入れ記録](acceptance-2026-10-08.md)、各モデル検証仕様 |
| 公開API | `manifest_path`、Module/Channel factoryの登録引数、callback、RunReportの項目をRust宣言に合わせた。`run`のDiagnostic互換変換と`run_with_diagnostics`／`run_config*`のRunFailureを区別 | [インタフェース仕様](../../specs/インタフェース詳細機能仕様書.md)、[run.rs](../../../crates/dir-simulator/src/run.rs)、[Registry](../../../crates/dir-simulator/src/registry/mod.rs) |
| CLI失敗報告 | 結果公開成功時の完全報告と、runの引数・予約・公開失敗時の7項目の簡略JSONを区別。後者は両パス・処理実時間がnullで、計数等は省略される | [CLI仕様](../../specs/インタフェース詳細機能仕様書.md#23-cliの字句標準出力)、[診断仕様](../../specs/診断詳細機能仕様書.md)、[main.rs](../../../crates/dir-simulator/src/main.rs) |
| schema・再現情報 | workloadの版を一律2とする説明をモデル別契約へ修正。台帳文書版、台帳SHA-256と構造・値・channel・実装範囲の来歴を記載 | [共通モデル仕様](../../specs/拡張モデル共通詳細機能仕様書.md)、[結果仕様](../../specs/結果詳細機能仕様書.md)、[再現情報の実装](../../../crates/dir-simulator/src/output/metadata/reproduction.rs) |
| AXI | 全Managerを一つのFIFOと読める説明を、Managerごとのread/write共有FIFOとManager間round-robinへ修正 | [AXI仕様](../../specs/models/AXIモデル詳細機能仕様書.md)、[runtime](../../../crates/dir-simulator/src/runtime/axi.rs) |
| 構成・Viewer | 16profileのソース配置、PreparedSimulation/Snapshotのtransaction・network状態、Viewerの必要JS一覧とprofile対象を更新 | [アーキテクチャ](../../アーキテクチャ設計書.md)、[Viewer取扱説明書](../../tools/viewer/viewer取扱説明書.md) |
| ガイド・ツール | Pages公開状態、CAN FD実習入力の入手元、既に公開されたNED editorの提供状態を更新 | [ガイド](../../guide/index.md)、[CAN FD実習](../../guide/CANFDのデータ速度比較.md)、[NED editor](../../tools/ned-editor/NED-editor仕様書.md) |

## 検証結果

| 検査 | 結果 |
| --- | --- |
| Rust `cargo test --locked -p dir-simulator` | 569件合格、失敗0、skip 0。台帳本文の最終修正を含めて再ビルド・再実行 |
| Rust format・clippy | `cargo fmt --all -- --check`、`cargo clippy --locked --all-targets -- -D warnings`合格 |
| JavaScript | 一括実行165件合格・任意browser gate 2件skip。その2件を既存Playwrightで別途実行し、両件合格。合計167件を確認 |
| Git管理下のPython文書検証 | 4ファイル・48件合格。ガイドscope policyも9件合格 |
| 全公開サンプル | 52入力・16profileのvalidate/run/view、156コマンド合格。manifestの208ファイルでサイズ・SHA-256を照合し、metadataの台帳hashも実在文書と一致 |
| CLI失敗契約 | 正常、出力先衝突、保存された準備失敗、run引数不正、validate引数不正を実測。終了コード・stdout項目・stderr診断が修正した説明と一致 |
| ガイド数値 | CAN調停3条件、受信フィルタ3条件、CAN FD速度3条件を実行。時刻・受信状態・独立変数・manifest hashを照合 |
| ガイド公開物 | MkDocs strict build、10 HTMLのローカルリンク・asset・anchor 463件、非公開執筆メモの除外、配布ZIPの11件／8件入力一致を確認 |
| 文書リンク・参照 | 22,246参照中、ローカル22,125件・fragment 18,959件を確認。未解決0。固定snapshotのルート相対参照9件は各snapshot基準で解決。生成search JSの3参照は実MkDocs生成物でも確認 |
| トレーサビリティ・図 | 編集可能な文書から再生成し、生成物の鮮度・strict検査に合格。231要件・60機能・274追跡node、構造エラー・不完全項目0。PlantUML 11図の検査合格 |
| モデルfixture | CAN、GW、AXI、SoC、メモリ・IPCの独立checker合格。製品実行とは別の検査として扱う |
| 原文・表示の対応 | 固定Rust42package、89原文、提供payload37件、ガイド8原文等の対応検査合格。`lock_bytes_equal=false`は添付lockと現行lockの末尾改行差で、package記録は一致 |
| 最終差分 | `git diff --check`合格。製品ソース変更0、既存文書の確定履歴表80件を保持 |

Pythonの単純な`unittest discover -s tests`は、Git管理対象外の旧OMNeT++比較試験も探索し、存在しないGateway fixtureで15 errorsとなった。元の失敗を保持したうえで、`git ls-files tests/*.py`で列挙した公開リポジトリの4試験ファイルを実行し、48件の合格を確認した。管理対象外の比較試験や欠けたfixtureは変更していない。

この監査は文書の整合性、実行した回帰試験・サンプル・リンク検査の範囲を示す。全受け入れ条件の承認、規格全文への適合、未実施のnative・性能・正式配布archive照合を完了扱いにしていない。個別の残条件は[引渡し手順](../native比較・配布照合引渡し手順.md)と各検証記録で管理する。commit・push・Pagesへの反映は行っていない。

## 文書改訂・push準備の追記

上記の監査件数・hash・試験結果・未commit状態は、`main @ 0b7b23d`を基準とした監査時点の記録である。その後、利用者の依頼に従い、リモート`main @ 1e33955`を取り込み、新しいガイド・Viewer機能・限定検証の証拠と監査修正を統合した。81文書のPATCHをpush済みの版（新規記録は監査時点の初版予定値）から一つ上げ、各文書の対象GitHubバージョンを`v1.1.4`に統一した。既存Gitタグ`v1.1.4`は維持する。改訂後の文書と生成一覧をcommit・pushするための準備・検証は、機械可読記録の`publication_preparation`とGit履歴で識別する。上記のsnapshotを現在のソースで再実行した証拠へ読み替えない。

統合後の版確定ではRust workspace 610件、Git管理下のPython検証49件、MkDocs strict build、ガイド15 HTML・800ローカル参照、7実習ZIP、format・clippy・11図の検査が合格した。改訂後の台帳文書版1.1.2とSHA-256が実製品のmetadataへ反映され、manifestの4収録ファイルもサイズ・SHA-256一致を確認した。Viewerのタグ以後の追加操作は現行mainの機能として区別した。

## 文書版規約の適用訂正（追加push）

前push `7ee854f` は5文書の対象タグをv1.1.4へ変更したが、文書版の上位2要素を1.0系のままにした。本pushではNED詳細機能仕様書・設定詳細機能仕様書・NED-editor取扱説明書・CANモデル検証仕様書・入力・設定検証仕様書を1.1.0へ訂正する。対象MAJOR.MINOR変更時はPATCH 0という運用規約を適用し、前pushの1.0系履歴行は変更・削除しない。

本記録の初回pushは新規文書にもかかわらず1.1.1を付けたため、初回PATCH 0の規定と不整合だった。この事実を訂正記録として残し、push済みの初回履歴を保持する。本記録は今回既存の1.1系文書として更新するので、追加pushの通常規則で1.1.2とする。初回履歴を1.1.0へ書き換えたものでも、初回版を正当化したものでもない。元の監査snapshot、81文書改訂の実施事実・試験結果、前pushの機械可読記録を維持し、今回の版整合訂正はその後の段階として区別する。

今回の変更はこの6文書の冒頭版・履歴追記・本節と、機械可読追記および生成一覧のみ。製品コード・サンプル・既存修正・Gitタグ・Release・workflowは変更しない。今回のローカル実施結果と未実施範囲は機械可読記録の `version_rule_followup` とPRに示す。
