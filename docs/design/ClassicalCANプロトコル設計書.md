# ClassicalCANプロトコル設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`design-can-protocol`
文書状態：実装ブランチ。v1.2.0収録。規則別検証済み、統合レビュー対象。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="protocol"></a>

```trace
{"id":"design-can-protocol#protocol","stage":"design","requirements":["DIR-REQ-0287","DIR-REQ-0288","DIR-REQ-0289","DIR-REQ-0290"],"upstream":["architecture#can-protocol"],"state":"confirmed","pending":[]}
```

`runtime::can_protocol::register_can_protocol` registers a real `Model`, one
bus arbitration resource, strict profile input/event/record validation, count
metrics, cooperative-control capability and a versioned explicit checkpoint
codec. It uses the existing Registry/Context arena and leaves legacy CAN engine
and input semantics intact. Central integration requires only the runtime module
declaration and default Registry registration call.

The model-config and workload are typed structures under
`src/lib/types/can_protocol.rs`; their public module is
`runtime::can_protocol::types`. The matching input/output implementations are
nested under the new runtime module to avoid a parallel central dispatch path.
`codec.rs` owns encoding and independent received-bit parsing;
`confinement.rs` owns counter transitions; `random.rs` owns integer sampling.

A generator callback advances only its own explicit cursor/RNG and enqueues a
request. Arbitration starts each eligible controller's selected request and advances every contender through actual sampled arbitration bits. Losers become receivers immediately and keep their request/attempt lineage; identical IDs remain concurrent through data and CRC. Every subsequent bus bit is an actual scheduled callback. Individual
controller active flags drive dominant bits; receiver-local faults alter only
the corresponding sample. Sender mismatch and receiver parsing create individual
flags, which can induce secondary detector flags and physically overlap.
Each controller owns its flag, delimiter, frame/intermission/suspension phase and local frame origin. Request failure settlement is idempotent and occurs before next-SOF eligibility; physical occupation closure is a separate operation. Flags survive replacing an aggregate accounting attempt. Receiver validity commits at penultimate EOF independently of transmitter final-EOF success. A damaged delimiter restarts error signalling or starts overload at its specified boundary. The same per-bit callback samples frame, flag, delimiter and intermission, including external SOF and scheduled faults. Counter rules use the state before the error to select the flag type. Bus-off recovery
consumes the same actual bits as all other bus observation.

Callbacks retain a private pre-callback state copy and restore it on a callback
error, while Context collects effects through the shared registered arena.
Time arithmetic is checked. Active-attempt bit deadlines derive from immutable
SOF plus a rational bit count rounded once to picoseconds, avoiding per-bit
rounding drift inside a frame. Quiescent idle recovery likewise retains an idle origin and cumulative bit count
so successive idle samples do not accumulate per-bit rounding drift.

An application FIFO is separate from physical reception. One item owns the
service timer; completion releases that slot and schedules the next item. A
checkpoint stores the queue, and the registered arena stores its pending timer.

Cancellation is a real scheduled event. It removes a queued request, records a future request selector without inventing generation, or marks the active request so only later retry is suppressed. Physical completion is never undone. Timers are retained with their event payload/deadline and checked against the registered runtime's owned live timer/reservation sets during encode/restore. Every event removes and validates its owned timer before changing state; callback rollback restores the timer and model state together.

The selected editions and finite mixed/Gateway execution contracts are in [仕様書](../specs/models/ClassicalCANプロトコル仕様書.md). Rule vectors and continuation/tamper evidence are in [検証仕様書](../verification/cases/ClassicalCANプロトコル検証仕様書.md). This profile models an ideal digital bus, without electrical, oscillator or analog conformance claims.

`can_protocol/mixed.rs` coordinates compound buses. `can_protocol/time.rs` provides checked reduced rational absolute time; `canfd/receiver.rs` incrementally parses actual prefixes, without receiving an expected frame. Each node owns a drive, parser, local phase and next boundary/sample. One registered timer publishes the minimum exact private event. Equal exact instants settle controls/drives, resolve wired-AND, freeze all node samples, then advance node phases. Distinct exact instants that ceil to one integer picosecond retain causal delta order. No globally precomputed FD phase switch is used.

The live state contains pending requests, parsers and finite Gateway queues; completed sample/record history lives only in Context output. Callback rollback clones live mutable state, not immutable generator/route arrays or accumulated output history. Immutable route definitions remain configuration; queued ingress stores a version plus matched indices. Epoch ancestry remains until every associated parser leaves its frame, even when the source transmitter failed locally. Arbitration losers withdraw from that physical source set.

`can_protocol/gateway.rs` consumes receiver completion, snapshots route version, schedules finite serial application service and converts each branch into normal downstream admission. Re-encoding supplies the rewritten wire plan and CRC. Capacity, conversion and hop-limit rejection preserve sibling branches. Route update controls run before same-time valid RX, and all generated/admitted requests run through ordinary arbitration.

CC implementation/codec2 refuses old aggregate layout. The explicit safe-state `migrate_v1_state` seam supports lossless old normal-frame/idle migration and rejects ambiguous post-frame states. Compound implementation/codec1 validates local queue/clock ownership, reverse generator reservation, native request identity, route versions, ingress service, parser/rate agreement and actual saved FES envelope identity. Restore runs no initialization or callbacks.
