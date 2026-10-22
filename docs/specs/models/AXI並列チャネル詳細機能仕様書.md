# AXI並列チャネル詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`spec-axi-concurrent`
文書状態：基点からの追加実装。v1.2.0収録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="channels"></a>

```trace
{"id":"spec-axi-concurrent#channels","stage":"spec","requirements":["DIR-REQ-0283","DIR-REQ-0284","DIR-REQ-0285"],"upstream":["DIR-FUNC-0075"],"state":"confirmed","pending":[]}
```

Profile: `axi4.concurrent.v1`. Implementation: `fabric.Concurrent` version 1.
Baseline vocabulary: Arm AMBA AXI4, IHI 0022C. This profile models synchronous digital channel handshakes and byte memory. It does not claim complete AXI electrical, reset, cache, protection, QoS, clock-crossing or standard conformance. AXI5 atomics are outside this AXI4 profile. The existing `axi4.transaction.v1` profile retains its prior contract.

## Public preparation and execution

Run `dir-simulator run --config examples/transaction-fabric/scenario.ini --output <new-directory>` using the CLI syntax reported by `dir-simulator --help`. Public `prepare`, `run`, and `RunSession` use the default registered profile. The NED network contains exactly one simple model with `@class("fabric.Concurrent")`. All real manager, interconnect and target ownership is declared in its validated JSON input; these are not independent completed simulations concatenated into an output.

`model-config` schema is `dir.transaction-fabric.model`, integer version 1. Required fields: `cycle_ps` (positive integer), `data_bytes` (power of two, 1–128), `managers`, `interconnects`, `targets`. Optional `write_buffer_beats` defaults to 4 and is 1–256 per manager. Manager, target and interconnect IDs are ASCII identifiers. A manager has unique `id` and `routes`: target ID to nonempty ordered, acyclic interconnect-ID path. Every target has one route per manager; unknown IDs, repeated path entries, missing routes, overlapping regions and overflowing addresses fail preparation. Each interconnect owns one independently arbitrated token for each channel, as do every directed link, target and manager. All paths are cut-through in one model cycle; this profile has no per-hop propagation delay.

A target contains `id`, integer `base`, positive integer `size`, `kind`, integer `read_latency_cycles`, integer `write_response_cycles`, and optional initial bytes. Unspecified bytes are zero. Per-region storage is limited to 16 MiB and total storage to 64 MiB. `kind` is descriptive (`sram`, `ddr`, `shared`, `mailbox`); every region currently implements the same byte-RAM access contract. DDR bank/JEDEC scheduling, ready-slot IPC ownership, notification delivery and integration with the legacy `memory.ipc.transaction.v1` service are **not** implemented by these kind labels. Therefore the architecture ledger's complete M01 adapter remains open.

`workload` schema is `dir.transaction-fabric.workload`, integer version 1, with `requests`. Each request has unique `request_id`, declared manager `source`, unsigned 32-bit AXI `id`, cycle-aligned integer `at_ps`, `operation` (`read`/`write`), integer `address`, power-of-two `size_bytes` no greater than bus width, integer `beats`, `burst` (`FIXED`/`INCR`/`WRAP`), optional Boolean `exclusive`, and optional timing controls `address_delay_cycles`, `data_delay_cycles`, `response_stall_cycles` (defaults zero). `transport` defaults to `axi`; `dma` selects a direct service adapter using exactly the same byte owner and exclusive invalidation path. DMA is a byte-copy request source, not the legacy IPC DMA engine. Workload order resolves equal issue times deterministically; earlier issue time wins arbitration before file order.

## Channels, backpressure and ordering

AW, W, B, AR and R each own separate resources. AW and AR handshakes can overlap a W or R beat and other managers on disjoint paths. Address and data delays are independent; W can be accepted into finite manager buffering before AW, but no byte commits before its address handshake. When its buffer fills, that manager's W channel stalls until a beat commits. W bursts from one manager follow write-request issue order with no interleaving, including distinct IDs. AW follows that same order. AR preserves address order within a manager/ID and can bypass a delayed address of a different ID.

A target commits at most one write beat per cycle. An address-accepted buffered beat consumes routed memory-write resources before its bytes become visible. A selected R response samples bytes when VALID is first asserted, then holds its payload and routed channel unchanged until the R handshake. Within a cycle, address handshakes occur first, W acceptance and write commits next, and R/B handshakes last; a read newly asserted in that cycle observes the new bytes. This is the model's deterministic same-cycle policy.

R and B preserve completion order per `(source, id, operation, transport)`, including different targets. Cross-ID responses can complete out of order according to actual target latency, contention and receiver stall. R interleaving between different IDs is allowed. Target read latency and write-response delay are each at least one model cycle, even when configured zero. Receiver stall adds that many cycles before each R beat or B response. B requires accepted AW, all accepted W, all corresponding memory beat commits, and response latency. Arbitrated responses retain source and ID; no electrical VALID/READY signal values are exported.

## Bursts and byte lanes

INCR supports 1–256 beats, FIXED 1–16, and WRAP 2/4/8/16. WRAP requires transfer-size alignment; wrap boundary is `floor(start / (size*beats)) * (size*beats)`. INCR's first unaligned transfer covers only bytes from `start` to the end of that transfer-size-aligned block; following addresses are `floor(start/size)*size + beat*size`. FIXED repeats that same byte range. WRAP addresses wrap within the total burst span. No burst may cross 4 KiB or a mapped-region boundary. An entirely unmapped burst completes with DECERR and no byte mutation; partially mapped bursts are rejected in preparation.

Each write beat supplies `data` and `strobe` arrays of exactly `data_bytes` elements, with bytes 0–255 and Boolean enables. Data is in actual bus lanes: lane for address A is `A % data_bytes`. Enabling a lane outside that beat's legal byte range fails preparation. Disabled bytes retain their prior values. Read data uses the same actual bus lanes and zeros inactive lanes. R and B reserve their routed response channel while VALID is held under receiver backpressure. A write provides exactly `beats` data entries; a read provides none.

## Exclusive monitor contract

An exclusive is an INCR burst of at most 16 beats and at most 128 bytes, with a power-of-two total range and a start aligned to that total range. The monitored granule is **exactly that total byte range**. The monitor key is `(source, id)`; accepting a new exclusive AR replaces that key's previous monitor with an inactive candidate. Only the actual last R handshake grants it, provided no intervening write invalidated the candidate. Any enabled byte write from **any** writer, including ordinary same-source writes and DMA, invalidates every overlapping monitor. Disjoint writes do not invalidate it. Reads return EXOKAY when the mapped target supports the exclusive contract.

At the first committed exclusive W beat, the service consumes the matching `(source,id,address,total_bytes)` monitor and saves the decision for that write burst. Success writes its enabled bytes and returns EXOKAY. Failure consumes no memory writes and returns OKAY. Decode errors return DECERR. Exclusives do not acquire an AXI lock or provide AXI5 atomic operations. Software workloads must await their exclusive read response before issuing the corresponding write; the simulator does not execute a software instruction stream for them.

## Records, stop and checkpoint

Schema 3 uses declared `fabric.Record` version 1 in `simulation.model_records`. `kind=channel` contains source, AXI ID, transport, request ID, accepted channel/COMMIT, optional beat and target, and actual detail. Time and address/base quantities are decimal strings; `effect_seq` preserves same-cycle ordering. Each record's `time_ps` is the actual commit time. A `COMMIT` states address and whether bytes were written, including failed exclusives and DECERR. `kind=memory` contains a target's latest committed bytes; it is a final-prefix state record, not reconstructed historical contents at earlier Viewer cursors. The generic Viewer exposes declared records without synthesizing absent responses.

At normal horizon H, only callbacks with time strictly less than H execute. H=0 emits no handshakes. Pausing/stopping mid-burst retains already committed bytes and actual records, with no fabricated B or remaining R. Continuation checkpoints persist all transaction counters, accepted/buffered W data positions, readiness times, stable asserted R payloads and response channel reservations, exclusive decisions, shared bytes/monitors, record sequence, and owned live tick timer. `fabric.state` codec version 1 verifies immutable model/workload identity, region sizes, counters and timer ownership before publication. Restore schedules no replay from time zero. Ordinary strict session checkpoint identity checks remain in force.

## Coverage boundary

This vertical increment addresses architecture A01–A05 and A07 under the above transaction model contract. It provides actual shared-byte AXI/direct-DMA visibility, but does not complete M01's legacy DDR/SRAM/shared/mailbox service bridge, A06 atomics, cache coherence, or a whole-SoC claim. IHI 0022C is the selected terminology baseline; official PDF retrieval timed out during this implementation, so test vectors here are independent handwritten expected behavior rather than a clause-complete normative certification.

Primary reference: [Arm AMBA AXI specifications](https://www.arm.com/architecture/system-architectures/amba/amba-specifications), [IHI 0022C AXI4](https://documentation-service.arm.com/static/5f915971f86e16515cdc34a6).
