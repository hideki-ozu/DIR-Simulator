# Explicit network extension profiles

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.6`
文書ID：`example-network-extensions`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-17` | 合成・抽象冗長配送・抽象preemptionの明示profileと実行手順をv1.1.6へ収録 |

Run from the repository root:

```sh
cargo run --locked -p dir-simulator -- run --config examples/network-extensions/composite.ini --output /tmp/dir-composite
cargo run --locked -p dir-simulator -- run --config examples/network-extensions/redundancy.ini --output /tmp/dir-redundancy
cargo run --locked -p dir-simulator -- run --config examples/network-extensions/preemption.ini --output /tmp/dir-preemption
```

- `composite.ini` connects Classical CAN conversion to the shared dynamic Ethernet and TSN queues. The synthetic ingress uses the Gateway Endpoint's `.rx` path for one PSFP evaluation per converted offer.
- `redundancy.ini` sends one origin along two explicit circuits through the triangle topology. Both consume the actual queues and links; recovery delivers the origin once and records the later branch as a duplicate. Paths share source and destination ports, so this example does not claim disjoint physical routes.
- `preemption.ini` fragments a 100-byte MAC frame into 40/40/20-byte slices with 4-byte abstract overhead. A legal 64-byte express frame arrives at 100000 ps. Fragment intervals are `[0,352000)`, `[1032000,1384000)`, and `[1384000,1576000)` ps. Express EOF is 928000 ps and release is 1024000 ps; the resume gap is `[1024000,1032000)` ps.

Each configuration selects one explicit profile. Replication and preemption are DIR abstract models, with no IEEE FRER or MAC Merge conformance claim. The usual `prepare` and `run` API accepts the same INI files. Results use schema 2 and keep planned and actual milestones separate at a half-open time limit.

Current source contains 20 builtins (16 existing plus these three and the wire-control profile). These extensions are implemented locally and are not part of a newly published release. Network cooperative control does not imply persistent checkpoint support; network checkpoint returns `UnsupportedCapability`. See [settings/API/Viewer boundaries](../../docs/guide/reference/後続10分野の現行実装と利用境界.md) and the parent-owned [product report](../../docs/verification/results/extension-product-2026-10-17.md) for final validation scope.
