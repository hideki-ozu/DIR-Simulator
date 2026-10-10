# Dynamic Ethernet fixtures

文書バージョン：`1.1.0`（初版予定値・push時確定）
予定公開版：未割当
文書ID：`fixture-network-dynamic`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-07` | 抽象ネットワークprofileの独立期待値と実行例を追加 |

Native examples live in `examples/ethernet/dynamic`. `expected.json` contains independent arithmetic and policy expectations; it is not an execution result. The unicast example uses a cyclic graph with a deterministic common tree. Additional workloads cover no traffic, IPv4/IPv6 source membership, router leases, registration refresh at expiry, and link switching while a copy is on wire.
