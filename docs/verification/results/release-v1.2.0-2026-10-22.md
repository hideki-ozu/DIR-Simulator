# v1.2.0収録内容・公開前検証記録

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-release-v120-20261022`
文書状態：公開用の収録範囲・文書採番・検証を記録。公開状態とCIの成否は対象タグ・Release・PRの実際の状態を参照する。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 追加81項目、公開用文書版、過去の検証snapshotと公開ソースの差分・build照合を記録 |

## 収録内容

v1.1.6以降のmainを基準に、実装commit `002a486fb7d4c7e0274d960174f5131afff7c060`の追加81項目を収録する。利用可能な組合せ・入力・判定根拠は[現行入口](../../implementation/README.md)と[実装台帳](../../implementation/remaining-capabilities.json)を参照する。

| 分野 | 項目数 | 追加内容 |
| --- | ---: | --- |
| CAN / CAN FD | 17 | 実bit列、誤り状態・再送・bus-off、Gateway変換、選択したISO FD契約と混在bus |
| Ethernet PHY / PLCA | 9 | 選択した1000BASE-T1・10BASE-T・10BASE-T1S/PLCA、実sample・codec/FEC・起動・fault回復、共有mediumとrepeater |
| clock / TSN | 6 | gPTP実packet、ローカル時計とTAS/CBS/PSFP、集中設定、half-duplex / PAUSEの適合条件 |
| bridge / network合成 | 13 | FRER・R-TAG・回復、MAC Merge、能動IGMP/MLD、RSTP/MSTP、provider、型付きネットワーク合成 |
| bus / NoC / SoC / memory | 27 | AXI/AHB実チャネル、NoC flit/VC、RV32I・MESI・MMIO・DMA・mailbox、DDR4 command/timing |
| 初期境界 / 制御・保存・結果 | 9 | zero horizon、legacy協調実行、checkpoint版移行、Metric V2、Viewer巻戻し |

既存の理想・抽象profileも保持する。規格名のあるprofileは、各仕様の選択した版・mode・数値精度・合成条件の契約を実装する。規格全体への適合、実機との相互運用、電気認証や未実行の性能条件へ判定を拡張しない。

## 文書版と過去の検証snapshot

公開済みmain `082629e1ecb3c966f60a043819c2362ef50e627f`をpush準備の基準とする。現行文書199件は対象を`v1.2.0`、文書版を`1.2.0`とした。MAJOR.MINORの変更に伴うPATCHの初期化であり、既存文書の公開済み履歴は保持し、未push分は一行へ集約した。[版一覧](release-v1.2.0-2026-10-22/document-versions.json)に対象・版・hashを記録する。

過去の測定結果、fixture、提案snapshot、固定commitを解説する参照章は元の基準を保持する。記事本文のv1.1.3/v1.1.4/v1.1.6等は、測定した版を示す。現行文書の版だけを更新して、過去の実行をv1.2.0の再測定と扱わない。

[追加81項目の検証記録](remaining-20261021/implementation-closure.md)、同じディレクトリの`validation.json`と`source-manifest.json`はcommit前に完了した検証snapshotである。「未commit・未公開」や`publication_state=uncommitted_unpublished`は当時の状態であり、公開の現在値を示さない。source manifestの672ファイルのhashは実装commitで参照できる。リリース準備による文書メタデータ更新後は、その古いhashを現在のファイルと同一と扱わない。

Cargo package / `metadata.runtime_version`は既存どおり`0.1.0`で、GitHubのソース配布版とは独立して管理する。実行binaryは`git_commit`、`git_dirty`、compiled source SHA-256、binary SHA-256、採用物台帳のhash/文書版で識別する。

## 検証の区分

実装snapshotのcompiled source SHA-256は`549d386cc0a15e99fc0c3e9073d7956d2935aa462289be62d4b067be3a08ac8c`。次の結果はそのsnapshotに対する製品検証であり、公開用buildのhashとは区別する。

| 検証 | 実装snapshotの結果 |
| --- | --- |
| Rust workspace | 95対象、1,820件成功・失敗0・通常除外59件。`cargo test --locked --workspace` |
| Rust整形・Clippy | 全target、warnings拒否で成功 |
| 実sample gPTP | 0/16/17sample伝搬境界で実startup・delimiter captureを確認 |
| 制御legacy / Viewer model | 14 profile×3段階を出力、142件成功・任意fixture 1件skip |
| standalone HTML | 6製品結果をdesktop/mobileで確認、巻戻し成功、browser error/external requestなし |
| 文書・guide | Python 67件、MkDocs strict、図13件成功。要件296・機能91・node414、構造error/不足0 |

公開用ソースでは、実装commitからRust実装・試験・Cargo依存関係の変更がないことを確認した。compiled sourceの423入力中、差分のある47件は`docs/specs`と採用物台帳のメタデータ・公開状態・更新履歴だけである。公開用ソースのcompiled source SHA-256は`cad9df90857ea7dee7d20e8db64df577d8d14c9062600521abd4a4dd6b75b952`で、独立計算と実CLIの出力が一致した。

| 公開準備で再実行した検査 | 結果 |
| --- | --- |
| release build / 実CLI | CAN、RV32I/MESI、SoC/DDR4の3入力で成功。source hash、台帳hash/版1.2.0、runtime版0.1.0、結果manifestの全ファイルhashを照合 |
| 文書・guide・配布入力 | 55コマンド成功。Python、正常/破損metadata、strict traceability、3生成物、図、MkDocs strict、相対リンク、ライセンス表示、入力ZIPを検査 |
| 過去証拠・入力 | 実装snapshot・提案・固定参照章・既存入力を保持。ZIP内の過去実習READMEも測定基準のまま保持 |

公開用buildのGit情報は公開準備前の実装commitとdirty状態を記録する。これは上記のsource hash・差分によって公開ソースへ接続し、まだ存在しない最終タグのcommitを測定したとは扱わない。初回検査で検出した同梱READMEとZIPの不一致、変更したメタデータ行の末尾空白は修正し、初回logと成功した再検査を別に保持した。公開状態の旧表記を整理し、AXI5の旧ファイル名への参照11か所を実在する文書へ修正した。整形途中のbuildと最終buildも別attemptとして保持し、最終仕様書と実CLIのhashを再照合した。

コマンド、成否、hashと差分の範囲は[機械可読記録](release-v1.2.0-2026-10-22.json)を参照する。保護されたmainへPRで取り込み、最終headのGitHub検査が成功した後、mergeのtreeを照合してannotated tagとGitHub Releaseを公開する。GitHub上での実際の成否は公開PRのchecksを参照する。

## 配布形式

GitHub ReleaseのSource code ZIP / tar.gzにソース、文書、仕様・設計、検証記録、入力サンプルを含む。Rust `1.85.0`を使用する。追加の実行binary、ローカルの試験出力、IEEE規格本文PDF/textは配布しない。第三者採用物・ライセンス原文は[採用物台帳](../../third-party/採用物台帳.md)を参照する。ガイドsiteはmainの検査通過後にGitHub Pagesへ公開する。
