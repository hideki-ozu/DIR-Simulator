# CANのAPI別受入条件と保存証拠

文書バージョン：`1.1.0`  
対象GitHubバージョン：`codex/ac0008-attributes-evidence-2026-10-10 @ f86268956e876ed1f528a45ce6cb21219d6451a5`  
文書ID：`can-acceptance-paths`  
状態：受入構成の変更を承認済み。個別確認と未試験を分け、AC0008全体は未充足。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
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

`python scripts/verification/can_acceptance_matrix.py --check`で保存証拠bindingと生成鮮度を検査する。`python scripts/verification/test_can_acceptance_matrix.py`は未試験状態・同値性とfactory証拠の分離・source/log結合を検査する。strict traceabilityと生成一覧、適用Python回帰、変更文書のリンク/anchorを別途検証する。これらは新たな製品実行ではない。
