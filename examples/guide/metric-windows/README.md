# Metric V2の集計窓実習入力

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.6`
文書ID：`example-guide-metric-windows`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-19` | 初版：固定Counterのmetrics-windowだけを4/7/10psに変える3条件 |

この入力の`demo.control.v1`は、正式v1.1.6の`crates/dir-simulator/examples/control_checkpoint.rs`が登録するprofileである。通常CLIの`run --config`では登録されない。

正式v1.1.6の全ソースを用意し、`cargo build --locked -p dir-simulator --example control_checkpoint`でexampleをビルドする。`target/debug/examples/control_checkpoint run <window-N.iniのパス> <未使用の出力root>`を実行すると`completed/results.json`等を保存する。その結果のViewerは通常CLIの`view`で生成できる。本実習は`run`だけを使い、checkpointのsave/restore実習ではない。

`reference/control_checkpoint.rs`は、正式タグ`397900451015c9bca6a33d24feac0a88b9b7b8fe`の同梱exampleをそのまま添付した照合用ソースである。このZIPは依存するRustライブラリ全体を含まないため、ZIPだけではexampleをビルドできない。NEDと条件別INIは相対参照を保って展開する。

Copyright (c) 2026 DIR Simulator contributors. Licensed under MIT OR Apache-2.0. リポジトリのLICENSE、LICENSE-MIT、LICENSE-APACHEを参照。
