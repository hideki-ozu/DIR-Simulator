# ClassicalCANプロトコル仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`（基点・未commit追加）
文書ID：`spec-can-protocol`
文書状態：実装ブランチ。未公開。規則別検証済み、統合レビュー対象。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。Bosch CAN 2.0 Bのbit実行、実仲裁、エラー・overload・confinement・retry・cancel、RTR、乱数生成、有限app RXとcheckpoint検証 |

<a id="protocol"></a>

```trace
{"id":"spec-can-protocol#protocol","stage":"spec","requirements":["DIR-REQ-0287","DIR-REQ-0288","DIR-REQ-0289","DIR-REQ-0290"],"upstream":["DIR-FUNC-0077"],"state":"confirmed","pending":[]}
```

Profiles: `can.cc.protocol.v1` (`dir.can.ProtocolV1`, implementation/codec version 2), `can.mixed.protocol.v1` and `can.gateway.protocol.v1` (`dir.can.CompoundProtocolV1`, implementation/codec version 1).
Status: executable finite ideal digital protocol contracts, with local CC phases, incremental FD reception and actual Gateway downstream bus execution. Integration review remains required.

## Standard and boundary

The edition target is **Bosch CAN Specification Version 2.0, September 1991,
Part B**, especially §§3.2.1–3.2.5 (frames/spacing), §6 (coding), §7 (errors)
and §8 (fault confinement). The original Bosch-authored document is available
at https://tech-tools.com/files/can2spec.pdf. This profile does not claim ISO
11898 electrical, oscillator, synchronization, transceiver or analog conformance.

One NED simple module has `@class("dir.can.ProtocolV1")` and no parameters or
ports. `examples/can-protocol/protocol.ini` is a runnable public prepare/run
sample. `examples/can-protocol/cancellation.ini` is the queued/active/future cancellation sample. The model-config contains `schema_version:1`, bitrate 1–1,000,000,
at least two uniquely named controllers, optional faults and overload selectors.
Input objects reject unknown fields. Controllers select listen-only behavior,
initial TEC/REC, automatic recovery, optional manual recovery-enable time,
remote responders and optional application receive policy.

## Frames and workloads

Frames explicitly contain `id`, `extended`, `remote`, `dlc`, and byte-array
`data`. Identifiers are 11 or 29 bits. DLC is 0–8; data-frame payload length
must equal DLC, while remote-frame payload must be empty independently of DLC.
Wire encoding calculates CRC-15 using polynomial 0x4599 and dynamically stuffs
the SOF-through-CRC region. A separate destuffer/parser checks samples and CRC.
The error-free data encoding is checked against the existing Classical CAN
encoder without changing legacy profiles.

A schema-version-1 workload contains uniquely identified generators. Generator
IDs are sorted before scheduling, each generator owns its RNG, and request IDs
are `generator:ordinal`. Each controller selects its pending request by arbitration field, generation time and stable request ID. All eligible controllers drive actual SOF/identifier/SRR/IDE/RTR bits concurrently. A controller sampling dominant while sending recessive in arbitration withdraws without an error counter penalty, becomes a receiver, and retains its request for later arbitration. Arbitration-loss attempt rows contain the actual observed prefix. Equal arbitration fields do not select a lexical winner: identical complete wires co-transmit; differing data/control/CRC bits cause the real bit/error rules to execute. A same-ID data frame wins
against a remote frame. A configured matching data responder produces a new
request retaining the remote request as `parent_request_id` after reception.

Schedules are tagged by these exact versioned kinds:

| Kind | Meaning |
| --- | --- |
| `explicit.v1` | Ordered integer-picosecond `times_ps` |
| `uniform.splitmix64.v1` | `count` emissions; first at `start_ps`, later interarrivals uniformly selected from inclusive `min_ps..max_ps` |
| `bernoulli.splitmix64.v1` | `count` trial slots starting at `start_ps`, separated by `tick_ps`; emit when the unbiased integer draw is below `numerator` out of `denominator` |
| `geometric.splitmix64.v1` | `count` emissions; each interarrival is the number of Bernoulli trials through the first success times `tick_ps`; the first trial is one tick after `start_ps` |

SplitMix64 uses the published 64-bit add/xor/multiply transform. Mapping to a
bounded integer uses rejection below `(-bound mod bound)` before modulo.
No floating-point transform is used. Cursors preserve seed-derived RNG state,
trial position, emission ordinal and current time; they do not precompute a run.
V1 bounds schedules to 100,000 explicit entries/emissions/trials, and geometric
sampling to 1,000,000 trials per emission, returning an execution error if that
sampling bound is exceeded. Arithmetic overflow is an error.

## Fault, recovery and application behavior

Faults select `request_id`, one-based `attempt`, `kind`, optional physical
stuffed `bit` index, and optional receiver-local corruption. A global `ack` inhibits receiver ACK driving and leaves the ACK recessive; receiver-local `ack` changes that controller's sample. Other kinds flip the selected sampled bit. ACK-driving receivers monitor their actual sample and only decrement REC when ACK sending succeeds. Their ACK eligibility comes from their independently parsed fields and verified CRC/delimiter, not a planned sender length. The resulting parser
or transmitter comparison determines the actual error class: a configured
`crc` label cannot force a CRC error when its bit actually violates stuffing.
Fault rows are canonically ordered. Active flags are driven by their individual
controllers and superpose on the digital wired-AND bus. Passive flags observe
six equal-polarity bits. CRC flags begin after the ACK delimiter; bit/stuff/form/
ACK flags begin at the following bit. Failed attempts retain request identity,
consume physical bus time, then retry through arbitration after delimiter,
intermission and any passive transmitter suspension.

Counters expose error-active/passive/bus-off boundaries, successful transfer
changes, the passive missing-ACK exception, arbitration-stuff exception,
flag-bit errors, dominant extension and receiver post-flag dominant detection.
A REC above 127 is deterministically reset to 120 on successful reception, a
value permitted by the selected edition. Bus-off nodes cease participation.
Automatic or explicitly enabled manual recovery counts 128 nonoverlapping
observations of 11 recessive bits from actual bus/idle samples before resetting
both counters; pending requests survive recovery.

Configured internal receiver conditions allow one or two overloads, beginning at the first expected intermission bit after the selected successful request. Every controller contributes a six-dominant-bit flag; physical overlap follows individual observations. Dominant samples at intermission bit one/two and at the last error/overload delimiter bit start overload at the next bit, without a counter increment. A dominant third intermission bit is interpreted as SOF; waiting eligible transmitters continue at their identifier, without sending a second SOF. A malformed external SOF is parsed and produces its actual error. The first recessive sample after a flag begins an eight-bit delimiter; corruption in its following first six fixed recessive positions re-signals an error, while corruption in its final position triggers overload. All flag/delimiter/intermission bits receive the same scheduled faults and counter rules as frame bits. Active and overload dominant-extension thresholds include the transmitted flag; passive extension starts after its observed six-equal-bit flag. A final three-bit intermission follows recovery. Data reception is not repeated.

An optional `application_rx: {capacity, service_ps}` adds an ordinary-controller
FIFO and serial application-service events. Capacity includes the item in
service. A full FIFO produces an application drop independently of the already
successful bus reception. Application records expose queued/completed/dropped
states. The configured capacity is at most 100,000; capacity zero drops all
application admissions. The FIFO and pending service timer survive checkpoints.

## Scheduled request cancellation

`cancellations: [{request_id, at_ps}]` selects canonical `generator:ordinal` IDs. A selector must name a declared generator and an ordinal within its finite schedule bound; unknown generators, leading-zero ordinals, duplicates, response IDs and out-of-range ordinals are rejected at prepare. For Bernoulli, the bound is the trial count and a selected future emission may never occur. Cancellation then remains `waiting_for_request`; no generated request is invented.

Cancellations are sorted by `(at_ps, request_id)` and scheduled at initialization before workload generation or live bit timers. Thus equal-time cancellation precedes generation and EOF sampling; ordinary event ordering remains deterministic. A future valid ID is acknowledged when the generator actually emits it. A queued request is removed immediately with `not_started`. An active request records `cancel_requested` and acknowledges that future retry is suppressed, while the current physical attempt and peer reception continue normally. A subsequent actual failure is retained and no retry is queued; an actual success remains `completed` with `cancel_requested=true`. A cancellation after successful physical completion is `too_late`. Cancellation records and physical attempt outcomes are separate. Live cancellation timers, processed IDs and terminal outcomes are checkpointed.

## Observation and control

Schema 3 uses `dir.can.ProtocolRecord` version 1 model records for requests,
attempts, controller counters, error flags, overloads, RNG cursors, bus reception
application admission/service and cancellation acknowledgements. Successful receiver reception is emitted at the observed penultimate EOF bit; transmitter success waits for the final EOF bit. A dominant final EOF bit causes receiver overload without erasing its already valid reception; an unfinished horizon never invents completion. Attempt rows
contain actual observed bits on completion/failure, rather than subtracting a
configured delay. Integer point metrics count attempts, failures and receptions.

The existing registered runtime supplies cooperative control, actual committed
prefix export and explicit checkpoint bundles. Codec `dir.can.protocol.state`
version 2 stores live per-controller phases and flags, per-bit samples, queues, counters, recovery,
application FIFO, RNG state and every owned timer/event/deadline. Restore validates counter/range/source/request/peer/flag/sample consistency, exact timer references and registered live reservations before publication;
it does not replay initialization or calculate a completed future prefix.

## Controller-owned progress and checkpoint versions

Each controller advances its own frame, flag, delimiter, intermission and passive suspension. A receiver-local second-intermission overload does not freeze a peer's third-intermission SOF. Local dominant SOF exists even when resolved peer samples remain recessive. A failed request is settled and made retry-eligible once before third-SOF joining; physical occupation closes separately and excludes the adopted SOF. Previous passive transmitters receive during their eight-bit suspension, while arbitration losers may join normally. Reception commits once per local parser.

The changed CC state layout has implementation/codec version 2. Version 1 is never silently decoded as version 2. `migrate_v1_state` transforms normal pre-completion frames and unambiguous unsuspended idle states. Aggregate old recovery/intermission/suspended states are rejected because individual local phase cannot be recovered losslessly. The migration fixture was generated by the genuine old version-1 production model and public RunSession checkpoint path. Checkpoint restore compares every owned timer with the actual saved FES envelope, including deadline, schema and payload.

## Mixed CC / ISO FD contract

The two compound profiles require `schema_version:1`, `standard_edition:"ISO11898-1:2015-wire/Bosch2.0B-cc"` and `execution_contract:"ideal-digital-can.v1"`. This names the project's finite digital contract; it is not complete ISO11898 certification. The ISO-style CRC/count wire representation is combined explicitly with Bosch2.0B confinement and 128×11 bus-off recovery. Count Gray-code/parity mismatch follows the project's CRC-invalid policy: withhold ACK, retain causes and signal after ACK delimiter. A form error may supersede that delayed error while preserving its causes. Full ISO edition equivalence is not asserted.

Each named bus has nominal/data rates, rational nominal/data sample fractions, and `cc_only`, `fd_capable` or `monitor_only` controllers. `fd_tolerant`, PXHD=0 reintegration and hardware emulation are rejected. `cc_only` continues Classical CAN interpretation at nominal timing; recessive reserved FDF does not immediately create a CC form error. FD reserved res violations use the declared PXHD=1-like form-error policy. ESI is derived from the transmitter's confinement state at actual attempt start, never from workload data.

The transmitter codec provides an intended wire plan. An independent bounded incremental receiver derives format, ID/RTR/FDF/BRS/ESI, DLC/data, dynamic/fixed stuffing, count and CRC from actual sampled bits. No expected payload or complete-frame replay supplies receiver truth. Transmitters drive ACK recessive. Only actual CRC/count-valid receiver prefixes request ACK; invalid or monitor-only receivers do not ACK. A lone transmitter therefore fails with missing ACK.

Each controller owns exact rational sample/boundary time. All same-exact-time drive changes settle before a frozen set of samples. BRS changes the remaining bit to data timing; the CRC delimiter and each immediate protocol error return that node to nominal timing at its own sample, so flags can start asynchronously. Let nominal/data periods be N/D and fractions sN/sD: BRS end is tSample+(1−sD)D, CRC delimiter/error end is tSample+(1−sN)N. Integer scheduler publication uses ceiling once per absolute exact time. Distinct exact times sharing a picosecond remain ordered through causal delta callbacks, without merging their physical samples. Arithmetic overflow fails execution.

Compound faults select canonical request ID, one-based attempt, `flip` plus bit, or `ack_suppress`, optionally a local controller. Global flips apply throughout the actual source-driven bit interval; local flips alter only that node's sample. Native and generated branch cancellation distinguishes future suppression, queued removal, active retry suppression and too-late completion; the active physical transfer still resolves normally. Live prefixes never add planned future completion.

Runnable examples are `examples/can-mixed-protocol/protocol.ini` and `examples/can-gateway-protocol/protocol.ini`.

## Gateway routes and exact conversion

A Gateway owns named ports, finite ingress capacity and serial service time. Only an actual local valid reception is routed. Route replacement at the same exact timestamp precedes reception, and reception snapshots immutable route version and matched rule indices. Later replacement cannot change a queued ingress item. Native downstream generation and Gateway service admissions precede ordinary bus arbitration at that timestamp.

Branches carry origin, physical parent request, hop count and route version. Rewritten ID/format is re-encoded, so downstream arbitration, stuffing, CRC, ACK, errors and retries are ordinary physical bus execution. Sender failure does not erase physical ancestry when a healthy receiver still receives the frame. Independent egress overflow rejects only that branch. Ingress overflow and finite hop bound have distinct outcomes.

CC data converts exactly to FD; FD payload up to eight bytes converts exactly to CC. FD payload above eight bytes is rejected with source size/destination limit, without truncation or implicit fragmentation. CC RTR to FD is rejected because FD has no exact remote-frame representation. Unsupported rewritten format widths are rejected. No CAN↔Ethernet conversion is implied.

## Acceptance boundary

Focused tests cover C01–C12 and F04/F05 within these declared finite contracts: local divergent phases, failed retry/suspension/SOF occupation, actual FD parsing/ACK/errors, rational clocks, checkpoint continuation/corruption, Gateway rewrite/version/branch/loop/conversion. Rule evidence and current test commands are in [検証仕様書](../../verification/cases/ClassicalCANプロトコル検証仕様書.md). Schema 3 uses the generic controlled Viewer; dedicated CAN bit-level presentation is separate. No propagation, oscillator, synchronization, transceiver, analog, full-ISO or full M_CAN conformance is claimed.
