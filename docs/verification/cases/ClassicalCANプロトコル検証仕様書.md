# ClassicalCANプロトコル検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`（基点・未commit追加）
文書ID：`test-can-protocol`
文書状態：実装ブランチ。未公開。規則別検証済み、統合レビュー対象。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。Bosch CAN 2.0 Bのbit実行、実仲裁、エラー・overload・confinement・retry・cancel、RTR、乱数生成、有限app RXとcheckpoint検証 |

<a id="dir-test-0132"></a>

```trace
{"id":"DIR-TEST-0132","stage":"verification","requirements":["DIR-REQ-0287","DIR-REQ-0288","DIR-REQ-0289","DIR-REQ-0290"],"upstream":["design-can-protocol#protocol"],"state":"confirmed","pending":[]}
```

Verification ID: DIR-TEST-0132.

Run the focused production-path suite:

```sh
CARGO_TARGET_DIR=/tmp/dir-remaining-implementation-target cargo test --locked -p dir-simulator --test can_protocol --test can_fd_receiver --test can_mixed_protocol --test can_gateway_protocol
```

The suite prepares actual NED/INI/JSON with the registered profile and executes
real Model callbacks. Its independent expectations include legacy error-free
wire equality, explicit CRC/stuff positions computed by a separate Python bit
encoder, handwritten counter boundaries, manually derived ACK-error overlapping
flag timing, integer SplitMix64 golden times, same-ID data/RTR priority, overload
spacing, and recovery after 128 observations of 11 recessive bits. It also checks
that bus reception survives a full application FIFO.

Checkpoint tests compare uninterrupted model records with disk checkpoint /
restore continuation at three live bit/RNG boundaries and during application
service. Observation-horizon tests require no completion before EOF. Generator
array reversal must leave output records identical.

The suite additionally checks every counter rule and exception through handwritten boundaries, real active/passive dominant-extension thresholds, ACK-driver monitoring, arbitration-stuff exemption, active-flag entry at passive thresholds, missing-ACK passive exception with and without dominant observation, and bus-off recovery under 130 queued competing frames. A local recessive-tail corruption delays recovery from frame128 to129.

Nine semantic checkpoint mutations rebuild valid content-addressed blobs, manifest hashes and codec references before restore. Position/sample length, wire CRC, missing/wrong-deadline/wrong-owner timers, counters, unknown request source and unknown state fields must fail in the model restore codec. The original bundle remains restorable. Checkpoint continuation also covers five live concurrent/error/overload boundaries and a future cancellation timer.

Cancellation tests cover queued removal, a future valid request, unknown/noncanonical/out-of-range IDs, active physical success and failure, equal generation/EOF timestamps, too-late cancellation and checkpoint-before-cancel ordering. A successful peer delivery is retained.

| Primary-source rule | Independent expected trace |
| --- | --- |
| Bosch Part B 3.2.3/7.2, ACK error overlap | For standard ID0x123/data1234, ACK52; flags53..58 and54..59; delimiter60..67; intermission68..70; retry release after71bits |
| Delimiter form re-signal | Corrupt bit62: simultaneous flags63..68; delimiter69..76; intermission77..79; failed occupation80bits, next successful TEC15 |
| 3.2.4 last delimiter overload | Corrupt bit67: overload68..73, delimiter74..81, intermission82..84; failed occupation85bits and TEC7 after retry success |
| 3.2.4/3.2.5 intermission | Corrupt first/second intermission bits61/62: next SOF158/160million ps at500kbit/s; corrupt third63: next frame uses that bit as SOF at126million ps |
| 8(4)/(5), overload flag bit error | Corrupt overlapping flag bit62: TEC+8/REC+8, no additional REC+1; no duplicate bus reception |
| 8(6), dominance extension | Active flags beginning53/54 with dominant through75: TEC23/REC24 after successful retry; passive six-equal-bit flag plus seven following dominant bits does not extend TEC, eighth does |
| 8(12), busy recovery | Each unmodified61bit frame+3bit intermission contributes one11-recessive sequence; pending recovery sender starts after128×64bits, or129×64 with local corruption |

The selected primary document is [Bosch CAN Specification2.0, September1991, Part B](https://tech-tools.com/files/can2spec.pdf), printed pages50–54 and59–63. Retrieved PDF SHA-256: `8c246bda4ee01a5e0517e59ba3d3ea3ef5860920bc2ff2659005c365e793ae38`. Expected timing/counter traces above are derived manually from those clauses and literal wire positions, rather than computed by the implementation under test. This suite covers the declared ideal digital contracts; Gateway/mixed-FD tests exercise C09/C10/C12/F04/F05 under the explicit finite project contract; electrical/full-ISO conformance is not claimed.

The additional local-phase vectors cover receiver-local second-intermission overload versus peer third SOF, local SOF while resolved bus is recessive, settled failed retry joining at140million ps with prior occupation excluding that SOF, and previous passive transmitter receiving while the arbitration loser joins at126million ps. Receiver-valid penultimate EOF and final-EOF overload remain distinct. Genuine production-v1 checkpoint migration accepts safe normal-frame state and rejects ambiguous recovery.

`can_fd_receiver` checks all DLC lengths, base/extended format, BRS, independent decoded content, partial prefixes, dynamic/fixed stuffing, count/parity and CRC deferred error timing. `can_mixed_protocol` checks actual receiver ACK versus lone transmitter failure, nominal-only CC interpretation, real same-ID CC/FD disagreement versus RTR arbitration, active/passive asynchronous error return, passive ACK exception revocation, ESI from confinement, cancellation truth, exact nonintegral clocks and distinct exact bus instants sharing one scheduler picosecond. The rational-clock oracle derives intervals analytically rather than rounding per bit. Dominant-extension bus-off must cease protocol participation and wait for enabled128×11 recovery before retry.

Public disk checkpoints resume byte-equivalent model records at BRS, CRC-invalid pending state and asynchronous error boundaries. Eight compound semantic mutations rebuild valid blob/manifest hashes before restore and reject altered clock/rate/parser/source/cursor/timer/epoch state. Existing CC semantic corruption and live divergent-boundary continuation remain covered.

`can_gateway_protocol` checks rewritten-priority downstream arbitration and literal re-encoded CRC, same-time route replacement with retained queued version, independent egress overflow, exact conversion/rejection, bounded real two-Gateway loops and ancestry of a valid local reception despite passive source failure. Default-registry CLI examples and generic Viewer export validate public integration separately from the custom focused registry.

The family focused gate currently comprises47 CC,4 receiver,16 mixed and7 Gateway tests (74 total); the integration report records final source-review and gate evidence. Parent owns whole-repository clippy, central traceability generation and broader regression gates. Tests establish the stated ideal digital contracts and do not establish physical/full-ISO certification.
