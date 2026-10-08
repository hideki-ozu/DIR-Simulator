# native比較・配布照合引渡し手順

文書バージョン：`1.1.1`
対象GitHubバージョン：`main @ d7cb386`
予定公開版：`v1.1.4`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.1` | `2026-10-08` | 添付監査ZIPの受領・Cargo原文とPopper通知の収録・出典固定を引継ぎ済みへ更新。native比較と正式配布照合の残件を維持 |
| `1.1.0` | `2026-10-08` | 固定commit・入力・反復・比較・証跡手順と配布残件を作成。ZIP v2の取得障害と台帳識別判定の修正・回帰検証を反映 |

文書ID：`native-distribution-handoff`

文書状態：レビュー中。これは実行手順であり、native実行の完了記録ではない。初回監査のWindows環境ではWSLのE_ACCESSDENIED、Git所有者不一致、native toolchain不足で製品実行0回。その記録を今回の添付反映で変更しない。性能wall/RSSは[品質・配布方針](../品質・配布方針.md)どおり参考計測で、必須合否へ用いない。

## 1. 対象と前提

対象製品commitは`d7cb3861f9ac56646ac2dbacf6f324b12f7fcb1b`、Rustはrust-toolchain.tomlの1.85.0、依存は同commitのCargo.lockで固定する。native Ubuntu 24.04 x86_64とUbuntu 24.04/WSL2を別環境として記録し、同じfixture・Registry・初期条件で両方を新規実行する。歴史的WSL結果は開発中ソース・違うbinaryも含むため、同じ入力名だけで新native結果と比較しない。

台帳・手順・比較スクリプトはPR #39に収録するが、製品は上記固定commitをcleanな別checkoutで構築する。手順スクリプトと証跡はcheckout外へ置く。今後別commitの製品を比較する場合は、同一完全SHAで両環境のbaselineを取り直し、スクリプトのTARGET・手順・版を一改訂として更新する。

通常Codex環境でリポジトリのAGENTS.md、.agents/skills、現行規約を最初に再確認する。Git/WSL/Libraryが権限拒否を返した場合は中止して障害を記録し、safe.directory一括解除・管理者化・認証／権限変更・別経路による拒否対象の取得をしない。

## 2. 同一入力と独立期待値

両環境で`docs/verification/fixtures/can/competition.ini`と`release-arrival.ini`をそれぞれ3回実行する。INIから参照するJSON、models/demo/Main.nedを含めfixture/can全ファイルのbyte数・SHA-256を保存する。binary・Cargo.lock・rustc -Vv・git commit/dirty状態も保存する。

| 入力 | 独立期待値 |
| --- | --- |
| competition | 要求2件。SOF順a:0,b:0。SOF 0,106000000 ps、EOF 100000000,200000000 ps |
| release-arrival | 要求3件。SOF順b:0,a:0,b:1。SOF 0,100000000,206000000 ps |

期待値は[DIR-TEST-0083](cases/利用フロー・品質検証仕様書.md#dir-test-0083)と[CAN解析期待値](fixtures/can/scenarios.json)による。イベント列をソートして比較しない。期待値へ照合するSOF projectionの並べ替えと、保存された全simulation/CSVの順序比較を区別する。

## 3. nativeとWSLでの実行

以下を各環境の適切なcheckoutで実行する。`AUDIT`はPR #39のsupportディレクトリからcheckout外へコピーしたスクリプトの場所、`EVIDENCE`は存在しないcheckout外の証跡ディレクトリ、`REPO`は固定commitのcheckout。各値は実際の環境で決め、他環境の絶対パスを流用しない。

```bash
git rev-parse HEAD
git status --porcelain
rustc -Vv
cargo build --release --locked -p dir-simulator
cargo test --locked --workspace
python3 "$AUDIT/native_correctness.py" selftest
python3 "$AUDIT/native_correctness.py" capture \
  --repo "$REPO" --binary "$REPO/target/release/dir-simulator" \
  --environment native-ubuntu --output "$EVIDENCE"
```

WSL側では最後の`--environment`のみ`wsl2-ubuntu`とし、同じcommit/lock/fixtureを使う。cargo testの全ログと終了コードを別保存する。captureは6 CLI実行のargv・終了コード・stdout/stderr・参考wall、input/lock/binary hashと全出力を保存し、manifestの4ファイルのbytes/hash、metadata参照と独立期待値を検査する。実行中にbinary/inputが変わった場合も失敗にする。既存証跡へ上書きしない。

各環境で`uname -a`、`cat /etc/os-release`、`lscpu`、RAM/SSD、実行時刻とthread/同時実行数を保存する。nativeはWSL/VMでないことを運用担当者が確認し、仮想化状況を記録する。captureのenvironmentラベルだけでnativeを証明しない。参考RSSは必要なら`/usr/bin/time -v`で別記録し、採否閾値を設けない。

## 4. 正規化・比較・合否

native/WSLの証跡を読み取り可能な場所へ通常の方法で集める。元の結果とmanifestは編集しない。

```bash
python3 "$AUDIT/native_correctness.py" compare \
  --left "$NATIVE_EVIDENCE" --right "$WSL_EVIDENCE" \
  --output "$COMPARISON_JSON"
```

- 同一環境の3回はJSON simulation全体、CSV全行・値・順序が完全一致。同一binary・metadataの固定条件も一致すること。
- 異環境でも整数ps・順序・ID・件数・状態・byte内容は完全一致。JSONは型付き比較を行い配列順を維持する。派生比率のJSON number／CSV value_kind=numberのみ絶対誤差1e-12以下または相対誤差1e-9以下を許す。
- CSVから除くのはrun_id列のみ。列の欠落・追加・順序と全row順を保持する。event_seq/effect_seqを除外せず、比較のためのsortを行わない。
- metadataのrun_id、wall時刻・性能、絶対入力パス、OS/CPU/targetやbinary hashは環境証跡として保存しsimulation比較から分離する。git commit、compiler、lock/source/input/config hash、Registry/profile、seed、window、初期状態等の条件はfingerprintで別確認する。条件不一致は「比較条件不一致」とし、再現性の合格／製品不良とは分ける。
- manifestがcompleteでmetadata参照が実在し、各出力byte/hashが一致すること。全CSV/JSONを照合し、独立期待値も一致して初めて本2fixtureの比較合格とする。

スクリプトのselftestは型・整数差・観測順・比率の許容差とゼロ近傍を故意に変えた比較器検査であり、製品の実行結果ではない。この2fixtureだけでDIR-TEST-0083全体を合格としない。同時刻偽モデルのEventKey、phase固定点、dirty資源ID順、次deltaとgenerator/登録順反転は既存Rust/acceptance_workflowの試験を別実行し、named test・source・結果を対応付ける。

## 5. 配布・台帳の引継ぎ

[採用物台帳](../third-party/採用物台帳.md)にはRust42件とガイド8件を登録した。初回Library取得はHTTP 403だったが、2026-10-08に利用者からローカル添付ZIPを受領した。原本を保存し、37収録ファイルのchecksum、42件の名前・版・lock checksum、89ライセンス原文、追加ソース表示、既存ガイド原文7件を照合して反映した。原文対応は[第三者ライセンス表示](../third-party/第三者ライセンス表示.md)と[再照合結果](results/release-native-distribution-audit-2026-10-08/license-originals-reconciliation-2026-10-08.json)に記録する。提供側の公式archive取得結果と今回直接確認した原文hashを分け、特定のライセンス経路の採用決定は推測しない。

Bootstrap bundleのPopper 2.11.8 MIT原文・Federico Zivolo表示をガイドへ追加した。lunr-languages 1.12.0の出典は固定commitへ更新し、非minifyソースを保持する。Font AwesomeはCSSのMIT、fontのOFL-1.1、現在未収録のSVG/JSアイコンのCC-BY-4.0を区別した。TinySegmenter原文も一致しているが、UMD/Snowballの追加由来確認と正式配布物への対応は残る。

requirements-guide.txtだけではMkDocsの推移版が固定されない。実生成環境のpip freeze/inspect、wheel一覧とhashを保存し、生成物へ実際に入る資産と生成器だけの依存を区別する。固定製品archiveの全member・bytes・SHA-256と、承認された台帳集合・LICENSE両本文・必要NOTICE/表示を過不足0件で照合する。input-only ZIPや過去の結果manifestを製品配布archiveの代わりにしない。

台帳が追加されたPRの製品でadoption_ledger_sha256、adoption_ledger_versionの実在文書への到達、manifest.metadata_refを再取得する。対象d7cb386のversion fieldはGit commitだったが、本PRでは台帳文書版を取得する。台帳なし／不正なhash・版でidentifiedとしない回帰試験をRust CIで実行する。歴史的not-present記録は保持する。全採用物の条件・最終配布・出自レビューが未充足ならAC0008は未充足のまま。

監査ZIPに収録されたunicode-identのUnicode-3.0全文、rustix/linux-raw-sysのCOPYRIGHT、crate別の追加表示を保存した。rustixのCC0-1.0由来はソース表示で確認できるが、独立CC0全文は添付にない。全ソース3,465件の照合証跡は固定d7cb386候補のもので、PR #39修正後の正式製品配布archiveの検査とは区別する。原文未取得を残件から外し、target別実配布集合・承認・archive同梱の検査を引き継ぐ。

## 6. 証跡と保存

取得環境ごとに生出力6組、capture.json、binary/lock/input hash、OS/CPU/toolchain/仮想化確認、テスト・CLIログを保存し、compare結果・条件不一致／未実施／合格を分ける。採用物では監査ZIP hash・全ライセンス原本・台帳版・配布archive inventoryと確認者／日を保存する。検証記録の実測値と結論を独立ブランチ・Draft PRで通常pushし、remote内容と正確なheadのCIを確認する。文書版・履歴・生成一覧はpush単位で一改訂にまとめる。

タグ・Release作成、マージ、デプロイ、Issueクローズ、OMNeT++比較、削除済み成果物の復活、CodeRabbit操作、外部エージェントへの新規依頼は本手順の対象外。
