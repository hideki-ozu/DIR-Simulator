# Ethernet wire control examples

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.6`
文書ID：`example-ethernet-wire`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-17` | 能動RSTP/MVRPと受動IGMPv3/MLDv2の入力例・実行手順・限定scopeをv1.1.6へ収録 |

`active.ini` connects three bridges in a triangle. Each bridge runs ForceProtocolVersion=2 RSTP and an MVRP Full Participant on all connected ports. Bridge s1 declares VID 20 locally. RSTP exchanges actual BPDUs; MVRP declarations propagate over the selected tree. Local declarations alone do not authorize data reception.

`passive.ini` injects independently encoded IGMPv3 and MLDv2 INCLUDE reports at `Net.s1.rx_a`. The bridge snoopers learn conservative source-filter leases. IP payload validation applies only to these control inputs. Ordinary data remains opaque and uses the existing `ip_multicast` metadata.

The examples reuse `../ethernet/dynamic/models`. The profile is registered in current source; run these INIs with the normal simulator CLI. The active horizon is 4 seconds; the passive horizon is 100 microseconds.

Inspect `simulation.model_records`: active protocol event/transition/policy records, passive membership/timer records, boundary RX records, and actual queued/started/completed control transmissions describe separate facts. A forwarding candidate or queued TX is not a completed reception. Network-owned FDB completion records identify only the scoped dynamic keys actually removed; static FDB entries are preserved.

Verification status is maintained in the implementation handoff. Protocol planner tests alone do not establish normal CLI/media/Viewer acceptance or full IEEE conformance.

Run from the repository root, with fresh output destinations:

```sh
cargo run --locked -p dir-simulator -- run --config examples/ethernet-wire/active.ini --output /tmp/dir-wire-active
cargo run --locked -p dir-simulator -- run --config examples/ethernet-wire/passive.ini --output /tmp/dir-wire-passive
cargo run --locked -p dir-simulator -- view --input /tmp/dir-wire-active/results.json --output /tmp/dir-wire-active-viewer.html
```

The implemented scope is selected IEEE 802.1Q-2022 CIST, ForceProtocolVersion=2, full-duplex point-to-point customer bridges, and MVRP context 0 FullParticipant. Every bridge in the MVRP physical domain must participate on every connected port. Passive IGMP/MLD does not generate host/listener/router traffic or select a querier. Unsupported compositions are rejected; network checkpoint is unsupported. See [current contracts and APIs](../../docs/guide/reference/後続10分野の現行実装と利用境界.md) and the parent-owned [product report](../../docs/verification/results/extension-product-2026-10-17.md) for final acceptance. This local implementation is not a release or full-standard conformance claim.
