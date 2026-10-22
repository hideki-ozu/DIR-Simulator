# AHBパイプライン詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
公開版：未公開。タグ・Releaseは作成していない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | Full AHB2の実行pipeline・burst・lock・SPLIT/RETRYと版付き状態保存を追加。初版予定値 |

文書ID：`spec-ahb-pipeline`

文書状態：対象commitは基点であり、本追加は未commit。未公開の実装契約。追加要件DIR-REQ-0286・機能DIR-FUNC-0076の担当範囲を定め、旧profileの契約を変更しない。

<a id="pipeline"></a>

```trace
{
  "id": "spec-ahb-pipeline#pipeline",
  "stage": "spec",
  "requirements": ["DIR-REQ-0286"],
  "upstream": [
    "DIR-FUNC-0076"
  ],
  "state": "confirmed",
  "pending": []
}
```

Profile `ahb.pipeline.v1`, implementation `ahb.Pipeline` version1. Status: unreleased source implementation. The existing `ahb.transaction.v1`, `soc.shared.v1` and `noc.xy.v1` profiles retain their contracts.

## Selected protocol and policy scope

The selected protocol is **AMBA Specification Rev2.0, Arm IHI0011A (1999), full AHB**. AHB-Lite and later AHB editions omit SPLIT/RETRY; they are not substituted for this contract. This profile executes the selected digital address/data pipeline and response behavior. It does not claim complete electrical, reset, protection, endian-configuration, BUSY-source, arbitrary HLOCK sequence or full AMBA conformance. Arbitration is the declared project fixed-priority policy; lock input describes one whole locked workload burst. These restrictions are policy subsets, not equivalence to all full-AHB systems.

Primary clause ledger:

| Source clause | Executed contract |
| --- | --- |
| IHI0011A §3.4, pp3-6–3-8 | Separate address/control and data ownership; one address phase overlaps the preceding transfer's data phase. HREADY wait extends the data phase and holds the following address; write data is stable. |
| §3.5, pp3-9–3-10 | First transfer NONSEQ, uninterrupted subsequent transfers SEQ; cancellation/default behavior uses IDLE. BUSY generation is outside this source workload contract. |
| §3.6, pp3-11–3-16 | SINGLE, INCR, INCR4/8/16 and WRAP4/8/16; beat-address progression and 1KiB burst boundary. |
| §3.7, pp3-17–3-18; §3.10, pp3-25–3-27 | Aligned HSIZE no greater than bus width; little-endian active byte lanes derived from address and size. Full-AHB2 has no AXI WSTRB signal. |
| §3.8, p3-19 | Nonoverlapping address decode with 1KiB minimum target footprint. |
| §3.9.2–3.9.5, pp3-20–3-24 | Wait cycles use OKAY with HREADY low; ERROR/RETRY/SPLIT each use low/high HREADY response pair. Speculative next address is cancelled to IDLE. ERROR policy cancels remaining workload burst, one of the allowed policies. |
| §3.11.1–3.11.5, pp3-28–3-34 | At most16 physical master tags, including the reserved dummy master; independent address vs delayed data owner; fixed-priority policy and early burst termination. Interrupted remaining beats use legal SINGLE/NONSEQ reissues. HLOCK setup is one cycle before first locked address; final locked data keeps an extra IDLE address reservation. |
| §3.12.1–3.12.4, pp3-35–3-38 | SPLIT masks only that manager until actual HSPLIT. RETRY retains normal arbitration priority without masking. One outstanding workload request per manager. RETRY targets are restricted to single-manager access, as required by §3.12.3's architectural condition. |
| Arm DDI0170A, AHB Example AMBA System TRM, arbiter ST_LOCKED/ST_LAST_LOCK/ST_SPLIT, pp4-22–4-23 | Locked RETRY retains its owner. Locked SPLIT grants only dummy/default IDLE until HSPLIT, then restarts that locked master. This reconciles full-AHB2 lock exclusion with SPLIT masking. |

Official sources: [IHI0011A PDF](https://documentation-service.arm.com/static/5f916403f86e16515cdc3d71), [DDI0170A example-system TRM](https://documentation-service.arm.com/static/5f1061300daa596235e7b232), [DDI0226A university-kit TRM, p4-15](https://documentation-service.arm.com/static/5f1070380daa596235e83057). The first document was downloaded and inspected during implementation; the latter official source's arbiter text independently confirms the locked SPLIT/RETRY behavior. These references support the selected rules, not a complete certification claim.

## Input schemas

The NED network contains exactly one simple model with `@class("ahb.Pipeline")`; the model-config owns the bus and all managers/targets. Public `prepare`, `run` and `RunSession` use the normal registered path. Example: `dir-simulator run --config examples/ahb-pipeline/scenario.ini --output <new-directory>`.

Model schema `dir.ahb-pipeline.model`, version1, has positive integer `cycle_ps`, power-of-two `data_bytes` from4 through128, nonempty `managers`, nonempty `targets`, optional `responses`. Manager IDs and target IDs are unique ASCII identifiers. A manager has `id` and unsigned32-bit `priority`: smaller priority wins, then manager list order. There are at most15 configured managers, using HMASTER tags1–15; tag0 is reserved for the dummy/default IDLE master. Each manager's workload requests are served in `(at_ps,file-order)` order. A higher-priority manager may interrupt an unlocked burst at a transfer boundary; resumed remaining beats are individually encoded SINGLE/NONSEQ, with their original requested byte addresses.

A target has `id`, `base`, `size`, optional `wait_cycles` (default0), and initial byte array (default empty, remaining bytes zero). Base and positive size are multiples of1024 and entirely within the 32-bit address space. Regions cannot overlap. Region storage is at most16MiB and total at most64MiB. Targets are byte RAM. There is no inferred DDR, mailbox, cache or legacy memory/IPC timing behavior.

Workload schema `dir.ahb-pipeline.workload`, version1, has `requests`. A request has unique nonempty `request_id`, declared `source`, cycle-aligned `at_ps`, operation `read`/`write`, `address`, `size_bytes`, `beats`, `burst`, optional Boolean `locked` (default false), optional `write_data`. SINGLE requires1 beat; INCR accepts1–1024; fixed INCR/WRAP forms require exactly4/8/16. Actual transfer addresses must be size-aligned, fit32 bits and remain within the initial 1KiB address block. WRAP spans `size_bytes*beats` and wraps within that aligned span. `write_data` has exactly one bus-width byte array per write beat, using actual little-endian lanes; reads have none. HSIZE/address select enabled lanes, and all other bytes retain their old values. Arbitrary per-byte AXI strobes are not accepted. Unknown fields, invalid sizes/alignment/counts and boundary crossings fail preparation, including future requests after the observation horizon.

A response rule has `request_id`, `beat`, zero-based `attempt`, `response` (OKAY/ERROR/RETRY/SPLIT), optional `wait_cycles`, and `split_release_cycles` only for SPLIT. Rules are unique by request/beat/attempt. SPLIT requires a positive release delay; other responses reject a release delay. The selected rule defines the slave's response state, not precomputed final output: real arbitration, holds, reissues and bytes still execute. Without a rule, mapped targets return OKAY after their wait cycles; unmapped addresses use the two-cycle default ERROR response. A RETRY-capable target may be accessed by only its one declaring manager across the workload; unsafe multi-manager RETRY target use is rejected rather than pretending the architectural condition is satisfied.

## Runtime and visibility

Each callback samples one active HCLK boundary. A cycle record shows the presented address/control phase and the current data phase separately. With no waits, address0 at0 progresses to data0 alongside address1 at1. HREADY low prevents address acceptance, preserves the exact following address/control slot and current write payload, and commits no memory. When HREADY rises with OKAY, current write bytes commit or current read bytes are sampled; the presented address becomes the next cycle's data slot. A read address accepted during a prior write's completion therefore reads that committed value when its own data phase completes.

Non-OKAY response cycle1 uses HREADY low; the next cycle uses the same HRESP with HREADY high and an IDLE address slot. The speculative following transfer is cancelled, never committed. RETRY/SPLIT reissue the same request and beat with incremented attempt, leaving completed earlier beats intact. Only an actual OKAY data completion updates bytes, so cancelled and retried beats cannot double-write. ERROR ends the request and cancels remaining beats. SPLIT removes its manager from arbitration; HSPLIT is emitted at the configured future cycle, restores eligibility on the following cycle, and may still wait behind a higher-priority manager or held address. RETRY never sets a split mask.

A locked request asserts HLOCK during a setup cycle, owns address arbitration through the final data response, and reserves IDLE while its final data completes. RETRY retains that lock. SPLIT retains lock exclusion but selects only the dummy/default IDLE master while the requester is masked; it cannot grant a competing real manager. The finite positive HSPLIT delay ensures a configured locked split can resume. Arbitrary multi-request locked sequences, slave-dependent deadlock outside this declared model, reset and analog behavior are outside scope.

## Output and continuation

Schema3 declares `ahb.pipeline.Record` version1. Cycle records include actual HREADY/HRESP, separate address/data phase objects and owners, beat/attempt, HTRANS/HBURST/HSIZE/HWRITE, acceptance/cancellation, HLOCK/lock reservation, actual HSPLIT, mask state, write payload, completed read data and actual byte-commit Boolean. Addresses and times use exact decimal strings. `effect_seq` orders same-cycle observations. Memory records contain latest committed target bytes. They are current successful-prefix snapshots, not historical memory reconstruction at an earlier Viewer cursor. The generic Viewer displays declared records and does not invent an absent data completion.

At horizon H only callbacks with time below H execute; H=0 has no cycles. Pause/Stop at a wait boundary exposes no pending write mutation. The version1 `ahb.pipeline.state` codec persists address/data slots, waits and response phase, transaction/attempt progress, byte RAM, lock setup/owner, split masks and future release times, prior-address continuity, sequence and live owned tick. Decode checks immutable input, counters/slots/response rules, lock/mask references, memory map and timer liveness before restore. Continuation executes the next real callback without replaying earlier cycles.

This closes the scoped pipeline/burst/lock and SPLIT/RETRY execution gaps S01–S03/S10 for the new profile and supplies real AHB byte read-after-write behavior. It does not retrofit byte payloads into the legacy SoC/NoC profiles or close universal SoC composition or complete M01.
