# PSFP stream gateの境界実習入力

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.6`
文書ID：`example-guide-psfp-gate`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-19` | 初版：stream gateのbase_time_psだけを1ps変える2条件 |

`gate-76000.ini`と`gate-76001.ini`を正式v1.1.6の`dir-simulator validate/run/view`へ渡す。モデルJSONの差は`tsn.streams[0].gate.base_time_ps`だけで、workloadとNEDは公開サンプルと同一である。JSON名を除いたINIも同一である。元ソースは`397900451015c9bca6a33d24feac0a88b9b7b8fe`。

Copyright (c) 2026 DIR Simulator contributors. Licensed under MIT OR Apache-2.0. リポジトリのLICENSE、LICENSE-MIT、LICENSE-APACHEを参照。
