# v1.1.6収録内容・公開前検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.6`
文書ID：`verification-release-v116-20261017`
文書状態：収録する機能・文書の版・検証対象を記録。公開状態とGitHub上のCI結果は対象タグ・Release・PRの実際の状態を参照する。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-17` | 後続10分野、Timer通知API、機能リファレンス、監査修正をv1.1.6へ収録し、文書採番・公開前の再検証範囲を確定 |

## 収録内容

既存の16 builtinに次の4 profileを加え、Registryは20 builtinとなる。

| profile / API | 機能・動作と境界 |
| --- | --- |
| `can.ethernet.tsn.v1` | Classical CAN、CAN↔Ethernet変換、dynamic EthernetとTAS/CBS/PSFPを単一FESへ接続する |
| `ethernet.redundancy.abstract.v1` | 静的な複製枝の独立送信と有限履歴による重複除去。IEEE FRER/R-TAG全体への適合を表さない |
| `ethernet.preemption.abstract.v1` | fragment境界でのexpress割込みとpreemptable frameの再開。MAC Merge/SMD/verify handshakeは対象外 |
| `ethernet.control.wire.experimental.v1` | 受動IGMPv3/MLDv2と、採用した能動RSTP/CIST・MVRPのadapterを選択する。wireとTSN/Gateway/冗長/preemptionの合成は対象外 |
| `RunSession` / Metric V2 / checkpoint | 協調Pause/Resume/Stop/Export、明示登録した整数指標・集計、全codecを備える汎用registered backendの同一binary別process復元。network checkpointは明示拒否 |

Timer通知tokenを公開APIで取得できる。既存入力、同期run、V1計測、schema1/2の互換性を保持し、制御exportとMetric V2にはschema3を使用する。詳細な設定・API・固定値・利用条件は[現行利用ガイド](../../guide/reference/後続10分野の現行実装と利用境界.md)、[採用仕様](../../specs/後続拡張採用詳細機能仕様書.md)、[採用設計](../../design/後続拡張採用詳細設計書.md)を参照する。

機能の採否・設定追加・仕様変更を検討するための[機能・動作仕様ガイド](../../guide/reference/DIR機能・動作仕様ガイド.md)を含む。基準7章の本文・画像は固定commitを保持し、v1.1.6の追加分は後続10分野の章で説明する。最新mainの5記事も保持し、通常navは39ページ、404を含む生成HTMLは40件、配布入力ZIPは24件である。

## 文書の版と過去証拠

[確定した文書版一覧](release-v1.1.6-2026-10-17/document-versions.json)は公開済みmain `033fccd`を基準とする。既存変更17文書のPATCHを一度だけ加算し、新規29文書は`1.1.0`とする。計46文書のうち、現行仕様等35文書はv1.1.6を対象とし、固定基準・過去検証11文書は元の対象基準を保持する。GitHubのPATCHと各文書の改訂PATCHは別に管理する。

[前回の監査記録](v1.1.6-readiness-2026-10-17.md)とそのJSONは公開準備を行う前のsnapshotであり、そこに記載された「採番未適用・公開未実施」はその監査時点の状態である。本書の版一覧と今回の検証を別に記録し、過去の文書・測定JSON・画像のhashや対象版を新たな実測として書き換えない。[従来の製品検証](extension-product-2026-10-17.md)の1,322 vectorの対応表と、今回の再実行の証拠を区別する。

Cargo package / `metadata.runtime_version`の`0.1.0`は既存の実行形式の識別子であり、GitHubのソース配布タグ`v1.1.6`とは別である。公開ソースはGitタグ、実行時のbuildは`git_commit`・`git_dirty`・build source SHA-256・binary SHA-256で識別する。

## 公開前の検証

| 検証対象 | 今回の結果 |
| --- | --- |
| Rust | workspace 1,038件成功・失敗0。通常実行で除外するnative 19件は別実行で19/19成功。fmt・build・clippy成功 |
| Node | 228件成功・失敗0・任意のMozc IME 1件skip |
| 実CLI / Viewer | 20受入コマンド成功、7 export、制御Viewerの7組をdesktop/mobileで確認、従来Viewerも成功 |
| 別process checkpoint | 中断なしと復元後の`simulation`がcanonical SHA-256 `0c045c290550d9b96b8ac0210007bb5b7b4683ffbeb7366ed48f1b8968bda3fb`で一致 |
| 文書・生成物 | 43コマンド成功。Python 67件・script 29件・license 19件、要件258・機能71・階層318のstrict検査、図13件、入力ZIP 24件、MkDocs strict成功 |
| ガイドbrowser | 131/131成功、desktop/mobile計78ページ読込・38クリック、console/request/HTTPエラー0 |
| 公開Markdown | 196文書・相対参照13,911件、確認された欠落0。生成時だけ存在する検索asset 3件は生成siteで実在を確認 |

今回実行したbinaryのbuild source SHA-256は`5a0ea90ec3b1d6f14f63f317fb8b06c8eaa43faff6d42ee168f86b5eae9afd78`である。これはコード・採用仕様・採用物台帳等の入力を識別し、最終タグのcommitとは区別する。実行時はmain `033fccd`に変更を重ねた状態であり、binary hashと詳細な環境を機械可読記録へ残した。

初回のnative検証では必要な`DIR_NED_RUN_TEST_BINARY`指定漏れにより6件失敗した。初回logと原因を`attempt_history`に保持し、正しいbinaryを指定した再実行19/19を受入結果とする。ガイドの従来52クリック計画中14件はホーム本文に対象リンクが存在しない無効なprobeであり、実際の38クリックと区別した。リンク調査は外部URLの可用性や公開Markdown全体のfragment妥当性を保証しない。ガイドsite内のroute/fragmentはbrowserで別途検査した。

コマンド・実行結果・環境・binary/入力/logのhashを[機械可読記録](release-v1.1.6-2026-10-17.json)に保存する。最終文書版を反映したソースでRust、Node、native隔離、別process復元、実CLI/Viewer、ガイド、文書・生成物の検査を行う。実行時のソースと最終タグの関係は同記録のbuild source hashと版一覧で確認する。GitHubの必須チェックはPRで検証し、mainへの取り込み後に同じソースのタグを公開する。

本検証は採用した限定scopeに対するものであり、IEEE等の規格全体適合、外部機器相互運用、新たな大規模性能測定を表さない。任意の日本語IME試験はprivate X11/D-Bus、Playwright、IBus/Mozcを用いる専用環境が必要であり、実施有無を結果に明記する。

## 配布形式

GitHub ReleaseのSource code ZIP / tar.gzは、v1.1.6タグのソース・仕様・設計・検証記録・入力サンプルを含む。Rust `1.85.0`でbuildする。ガイドsiteはmainの検査通過後にGitHub Pagesへ公開する。利用環境・動作の限定条件、第三者採用物とlicense原文はREADMEと採用物台帳を参照する。
