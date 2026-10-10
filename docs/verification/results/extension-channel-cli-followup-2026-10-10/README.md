# 拡張channelとCLI validate境界の限定回帰

文書バージョン：`1.1.0`  
対象GitHubバージョン：`PR #47 @ 17b5faf1e67cfb93abf87493657e1a5f70c9c0f3`  
文書ID：`extension-channel-cli-followup`  
状態：未コンパイルの試験コードと実行計画。対象WSL実行・通常CIは別欄で確認予定。

### 更新履歴

| 文書版 | 日付 | push単位の内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-10 | 初回push予定。4つの限定回帰と実行記録の分離を追加。旧PR #47の実行証跡は保持 |

## 観測する範囲

[PR #47](https://github.com/hideki-ozu/DIR-Simulator/pull/47)のmodel factory検査と実process CLI validate検査に、以下の4試験を追加する。旧証跡JSON・ログ・当時のnot_runは変更しない。

| exact test名 | 限定した観測 |
| --- | --- |
| `generic_channel_factories_receive_frozen_inputs_in_stable_order` | 2辺のChannelConfigのid/key/唯一のdelayパラメータ、3/7ps、prepare中factoryゼロ。prepare後のINI変更を読み直さない。channelの独立状態3→4/7→8。内部設計のallocation/initialize/finish/drop順 |
| `generic_invalid_channel_parameter_never_reaches_factories` | descriptor最大10に対する11ps overrideをE-0001/invalid_range/対象キー付きでprepare拒否し、factoryとlifecycleがゼロ |
| `generic_channel_factory_failure_releases_only_constructed_prefix` | 3辺a/m/zのz factoryがErr。構築済みm/aだけを逆順finish/drop、model factoryゼロ、prep_failed、allocation診断、イベント・recordゼロ。panicモードは対象外 |
| `cli_runtime_boundary_tests::cli_validate_does_not_enter_runtime_and_run_positive_control_does` | 実main.rsのexecuteを同一test crateで取り込み、正常/不正validateでruntime facade/builtin/CAN engine/handle入口ゼロ。run正例で各入口1、callback数と成功時committed_events一致、CAN request2 |

正例fixtureはイベントを生成しないため、committed_events=0だけをfactory未実行の根拠にはしない。factory入力・lifecycleの実観測を正の対照にする。ChannelConfigに存在しないprofile_input/subject/workloadを検査したとは主張しない。

factory順は内部設計の回帰であり、実装から独立した通信時刻oracleではない。辺ごとの状態とprepare/runtime分離は公開登録APIでの挙動検査である。observer callback内にはassertを置かず、runtime後にtraceとdiagnosticsを検査する。

CLI counterはcfg(test)だけで実入口に置く。in-processで本番CLIソースを取り込む試験であり、出荷binaryを計測したものではない。既存`classical_can_cli_validate_accepts_fixture_and_rejects_missing_config`は実processの終了状態・診断・出力なしの根拠として保持する。任意外部pluginのCLI挙動や全runtimeのconformanceへ拡張しない。

## 実行環境と証拠

対象環境は既存WSL Ubuntu24.04 LTS x86_64、cargo1.85.0。既知のWSLアクセス拒否を再試行・設定変更で回避しない。今回ローカルRustはnot_runであり、通常GitHub Actionsの成功が得られても対象WSLの合格へ読み替えない。

更新した[実行runner](../../../../scripts/verification/run_extension_acceptance.py)は`--lib`/`--test`を区別し、exact名の`... ok`と`1 passed; 0 failed; 0 ignored`を要求する。0選択は失敗。uname/os-release/rustc/cargo/HEAD/statusとCargo.lock/入力hash、stdout/stderr/exitを保存し、既存8試験と新4試験の後にfmt/workspace/clippyを検査する。toolchainを導入・変更しない。

```sh
python scripts/verification/run_extension_acceptance.py --output /absolute/new/output
```

[ローカル実行計画](local-execution-not-run.json)と[新しい限定証跡](verification.json)を参照する。全channel capability、finish失敗matrix、全profile、全仕様・AC0008、license採用/作者承認は未受入。merge・Release・Siteへの反映は行わない。
