# AXI並列チャネル詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-axi-concurrent`
文書状態：基点からの追加実装。未commit・未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。独立チャネル・複数ID・byte memory・exclusive・実行制御の追加実装を記録 |

<a id="channels"></a>

```trace
{"id":"design-axi-concurrent#channels","stage":"design","requirements":["DIR-REQ-0283","DIR-REQ-0284","DIR-REQ-0285"],"upstream":["architecture#axi-concurrent"],"state":"confirmed","pending":[]}
```

`input/transaction_fabric.rs` deserializes deny-unknown-field schema structs and validates width, addresses, paths, bursts, strobes and exclusives before model construction. `lib/types/transaction_fabric.rs` holds the public immutable wire-independent JSON contracts and beat-address/lane calculation.

`runtime/transaction_fabric/mod.rs` registers `fabric.Concurrent`, `fabric.Tick`, `fabric.Record`, profile preparation, cooperative execution and explicit `fabric.state` checkpoint codec in the existing registry. One execution owner holds a real routed resource graph and invokes one cycle at a time through `Model::on_event`. It schedules only the next cycle with pending work, or skips idle time to the next request arrival. No initialization run, final-state precomputation or prefix replay occurs.

The committed state contains one live transaction state per workload request, a shared `MemoryService`, record sequence and last cycle. A cycle clones that state, issues due requests, arbitrates independent address channels, accepts finite buffered W, commits write beats and arbitrates ready R/B. Asserted responses hold route ownership and sampled R payload until receiver acceptance; these reservations are checkpointed. Fallible time arithmetic and record/timer effects use the existing callback transaction batch. State is assigned only after building that cycle's effects successfully. The generic runtime owns event queues, observations, safe pause boundaries and output. W acceptance counters are distinct from committed-byte counters, so AW backpressure and partial writes survive checkpoints.

Channel resource keys include manager, every interconnect and directed path link, and target. They exist independently per AW/W/B/AR/R and memory commit. Arbitration follows request issue time and original file index. Holding one token during a cycle represents that resource's accepted beat; unaccepted transactions retain state for the next cycle. A delayed target on one path cannot monopolize another target's link or response channel. Same-source W and per-ID response-order checks prevent illegal association and completion reorder.

`runtime/transaction_fabric/memory.rs` is the sole byte owner used by AXI and direct DMA. Requests retain source, request ID, ID, address, payload and enables; responses retain request lineage. The memory write method preflights enabled addresses before modifying bytes, invalidating overlapping monitors at that same visibility point. The service is public for subsequent CPU/IPC adapters, but this commit does not couple it to the legacy private memory/IPC arena or implement DDR commands.

The codec includes the immutable parsed inputs and live state, then compares restored inputs to the prepared model. It checks region identity/size, transaction counters, exclusive monitor range/key uniqueness and live tick timer through the generic timer-reference API. The generic checkpoint bundle independently persists scheduled envelopes, snapshots and provenance. `output/transaction_fabric.rs` provides ordered record inspection; existing schema 3 publication and generic Viewer render actual committed records.
