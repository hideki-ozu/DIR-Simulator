# CANのAPI別受入条件と保存証拠

文書バージョン：`1.1.2`  
対象GitHubバージョン：`main @ b45515644dfc65c205216d564d5bf7ef00280622`  
文書ID：`can-acceptance-paths`  
状態：受入構成の変更を承認済み。個別確認と未試験を分け、AC0008全体は未充足。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-10` | 公開済みv1.1.4を保持する文書整合追記。初回5件のJSONを保持し、現在の6件を別の実行記録へ保存。AC0008 README版の生成一覧不一致を独立再現し、全文書更新後に再生成 |
| `1.1.1` | `2026-10-10` | 第2push。0080の残存genericライフサイクル条件と、連続する0081のfixture指定を整理。残存検索・回帰検査を追加 |
| `1.1.0` | `2026-10-10` | 初回push。06:09 UTCの利用者承認に従い、組み込みCANと汎用拡張を分けた受入条件・API別source/実測対応を保存 |

## 承認内容と責務

2026-10-10 06:09 UTCに利用者が、組み込みClassical CAN（BuiltinAdapter / Engine）と汎用拡張factory / callbackを別々に検証する構成を承認した。製品のfactory移行は要求しない。この決定は[DIR-TEST-0085](../../cases/利用フロー・品質検証仕様書.md#dir-test-0085)と[操作仕様](../../../specs/インタフェース詳細機能仕様書.md#extension-contract)へ反映した。全試験合格、AC0008全体、ライセンス採用、merge/releaseの承認ではない。

| 条件ID | API・責務 | 保存証拠の確認範囲 |
| --- | --- | --- |
| CAN-BUILTIN-CLI | CLI run→run_config→default Registry→BuiltinAdapter→CAN Engine | ac0008_pathsのCLI正常終了とsimulation同値 |
| CAN-BUILTIN-RUN-CONFIG | run_configは同じ既定登録経路 | RunReport正常終了とsimulation同値 |
| CAN-BUILTIN-PLAIN | 通常prepare→prepare_with_source、registeredなし→run→CAN Engine | registeredなし、2要求の解析SOF/EOF、simulation同値 |
| CAN-BUILTIN-EXPLICIT | prepare_with_registry(default)→adapter→Engine | registeredあり・is_generic=false、simulation同値 |
| CAN-VALIDATE-DIRECT | CLI validateは通常prepareで検証のみ | source確認。対応する個別実行証拠はnot_tested |
| CAN-EXPLICIT-RUN-CONFIG | run_config_with_registryは渡されたRegistry/profileで選択 | 既存concrete testを参照。named registry実行log未結合のためnot_tested |
| GENERIC-PREPARE | custom Registryの準備はdescriptor/config確定、実行factory生成前 | source確認。factory counter0の個別実行証拠はnot_tested |
| GENERIC-FACTORY-LIFECYCLE | custom factory/initialize/callback/finish/解放と失敗prefix | registryのCALLS/状態assertionを参照。個別実行log未結合でnot_tested |
| GENERIC-CALLBACK-EFFECTS | custom配送・channel・仲裁・callback効果commit | registryの具体値assertionを参照。個別実行log未結合でnot_tested |
| GENERIC-REJECTION-CALLBACK-01～04 | Issue44の3拒否例＋unknown property | normal実callback1/committed event1、counter reset後の拒否callback0の保存実測。全generic lifecycleへ拡張しない |

観測同値性はfactory呼出しの証拠ではない。Classical CANの通常prepareでは既定Registryを必須とせず、明示Registry APIでもadapter/custom profileの選択を確認する。汎用factory/callbackの生成・順序・効果はその実counter/traceとconcrete assertionで別に確認する。

## 正確なsourceと実測

基礎PR #43公開headは `f86268956e876ed1f528a45ce6cb21219d6451a5`。Issue #41の属性保持、Issue #44のreason修正と新旧WSL証拠をそのまま保持する。実測source/testは `9adf18faf1d0791360c4528fe1445b2560e395f8`、WSL2 Ubuntu24.04.4 LTS、Rust/Cargo1.85.0、x86_64-unknown-linux-gnu。今回のエージェント実行環境では製品を実行していない。WSLアクセス・Rust確認の再試行や設定変更は行わない。

[run-result.json](../ac0008-issue44-2026-10-10/run-result.json)のfocused32件にはac0008_pathsの4件がnamed成功として含まれる。[保存stdout](../ac0008-issue44-2026-10-10/logs/focused-tests.stdout.log)のSHA-256は `69d954cbf64a5b51be271c1f7203e20dbc202da6dba1edc35356d54fc80a52b3`。新対応表は公開byte hash、named結果、source/test commitを照合する。[runtime-bindings.json](../ac0008-issue44-2026-10-10/runtime-bindings.json)の実callback positive-controlは別の限定証拠として記録する。

実際の経路は `src/main.rs:107`、`src/run.rs:171/180/194`、`src/input.rs:816/822`、`src/registry/prepare.rs:13/17/73`、`src/runtime/registered.rs:1019/1024`、`src/runtime/engine.rs:348`を参照。`source-navigation.json`に公開headのGit blobを保存した。CLI validateの通常prepareはmainのvalidate分岐で確認する。generic prepareのModelConfig確定とgeneric runtimeのfactory呼出しを別段階として扱う。

`tests/ac0008_paths.rs`の4件をcompetition入力だけの限定確認として扱う。`tests/registry.rs`のcustom_payload_channel_timer_cancel_and_arbitration_execute_deterministicallyはcommitted3・配送2/5ps・record byte7、callback_failure_discards_every_effect_and_preserves_pending_currentはcommitted2/pending1/points1・recordなしをassertする。acceptance_lifecycle_failures_release_models_and_keep_frozen_prefixはallocate/init/finish/dropのCALLSと失敗時prefixをassertする。これらはsourceを読んだ具体的対応で、既存569/29集計からnamed実行成功を推定しない。registered_builtin_adapter_preserves_builtin_resultの入力はgateway/fanoutであり、Classical CAN competitionの代用にしない。

## 現在の判定と履歴の保持

[acceptance-matrix.json](acceptance-matrix.json)が13条件の機械可読対応表。保存証拠が結合した8条件はpassed_limited、5条件はnot_tested。API別受入構成の承認と、実測assertionの範囲と、sourceによる経路確認を別欄で扱う。

旧AC0008記録のliteral factory不一致、PR #42の初回未承認案、Issue #41/44修正前の失敗とreason不一致は歴史的事実としてbyteを変更しない。旧提案の受入構成だけを今回の承認で前向きに更新する。全atomic oracle、作者出自、個別採用承認、static link人口等の残件は維持する。固定artifact3705member/6manifest/42crateの全件監査・再取得・Release再構築を行わない。

依存順は #40 → #42 → #43 → 本文書PR。#43のbranchを編集せず、head f862689から独立branchで文書のみ更新する。main/worktree/製品source/保存実測は変更しない。既存CIのbase main限定条件のためstacked PRは対象外。CI0件を合格とは扱わない。

## 文書検証

第2pushでは0080のライフサイクルをCAN Engineの初期状態構築へ修正し、0081のfactory/initialize/callback/finish失敗注入はcustom Registry fixture、出力/I/O失敗は0080入力を使用可能と明示した。受入責務を削除せず、適用経路を区別する。5つの変更した正本文書で同種のfactory/CAN/全初期化条件を検索し、残るgeneric条件は明示custom経路として保持する。過去の実施記録は変更しない。

`python scripts/verification/can_acceptance_matrix.py --check`で保存証拠bindingと生成鮮度を検査する。`python scripts/verification/test_can_acceptance_matrix.py`は未試験状態・同値性とfactory証拠の分離・source/log結合を検査する。strict traceabilityと生成一覧、適用Python回帰、変更文書のリンク/anchorを別途検証する。これらは新たな製品実行ではない。

### 初回5件と後続6件を分けた記録

[verification.json](verification.json) の文書版1.1.0・5件成功は初回pushの歴史記録としてbyteを保持する。第2pushで追加したライフサイクル回帰を含む現在の6件は、今回、公開済みv1.1.4と同じmain `b45515644dfc65c205216d564d5bf7ef00280622` のscript・入力と照合した独立snapshotで再実行し、6件成功を確認した。[追記JSON](verification-followup-2026-10-10.json) と[名前付き6件のログ](followup-unit-tests-2026-10-10.log)は今回の実行時刻・Windows Python環境・source hashを記録する。過去の5件を6件へ書き換えたり、第2push当時のログを復元したとは扱わない。

同じmainの生成入力111ファイルを公開Git blobとbyte照合して`generate_traceability.py --check`を実行すると、MarkdownとHTMLの一覧がstaleになった。独立再生成の差分は、AC0008残余READMEの文書版1.1.2を一覧が1.1.1と記録していた1行ずつだった。このREADMEの追記と改訂を確定した後に一覧を再生成し、strict検査・生成鮮度を確認する。追跡構造とPython文書検査の結果であり、製品不具合や公開済みv1.1.4の停止理由ではない。
