# AXI5アトミック・QoS検証仕様書

文書バージョン：`1.1.0`（初版予定）
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
公開状態：未commit・未公開の実装作業文書。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 単一ownerによる実行・転送・復元契約を追加 |

文書ID：`verification-axi5`
文書状態：実装対応草案。受け入れ契約と実行済み証跡を区別する。

<a id="dir-test-0139"></a>

```trace
{"id":"DIR-TEST-0139","stage":"verification","requirements":["DIR-REQ-0300","DIR-REQ-0301"],"upstream":["design-axi5#atomic"],"state":"confirmed","pending":[]}
```

## 受け入れ群

`tests/soc_execution.rs`の`axi5_all_operations_signed_edges_compare_layout_and_independent_responses`はStore/Load全8op、Swap、Compare match/mismatch・上下operand配置、signed境界、R/B片側stall、writer interferenceを独立期待値と比較する。`qos_aging_and_admission_rotation_drain_finite_saturated_queue`は低QoSと連続高QoSの有限混在、admission時刻、effective上限15と全drainを検査する。

`tests/transaction_fabric.rs`の`shared_slave_keeps_aw_and_noninterleaved_w_order_while_first_aw_is_delayed`は先頭AW遅延・W buffer=1・二manager共有slaveでAW順序とW非interleaveを検査する。invalid AWATOP、Compare burst不一致、非aligned inputをprepare拒否する。atomic途中checkpoint/DDR物理write途中の再開とtamper拒否を共通SoC gateで確認する。

focused commandは`cargo test -p dir-simulator --test soc_execution`と`--test transaction_fabric`。実行済み結果は統合証跡へ保存する。全AXI5転送幅・endianness・coherent ACE機能を提供したとはしない。
