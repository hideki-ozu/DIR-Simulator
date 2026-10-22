# Goal

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`review-remaining-gateway-frer-20261021`
文書状態：未commit・未公開。選択範囲の独立レビュー記録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-22` | 実Gateway複合経路と保存・atomic検証の独立レビュー |

**No new P1/P2 found in the inspected N12 composition. Bounded N12 closure is supported with the explicit acceptance-scope decision below.** Product source was read-only. This verdict concerns actual Gateway-to-FRER integration and its tested supported composition, not full CAN/Ethernet/802.1CB/802.3 certification.

# Architecture

The actual Classical CAN receiver/conversion remains the source. A typed GatewayEgress selector binds gateway/rule/egress to a real admitted conversion child; no synthetic Ethernet generator is introduced. The child follows the ordinary physical prefix, vector bridge-pair FRER, CBS/TAS, actual MAC Merge branch serializer, accepted recovery round and actual physical suffix. NetworkDelta and the existing FES retain transactional ownership.

# Files

Inspected handoff/progress/design, runtime/can_ethernet.rs, runtime/network.rs, network/{replication,distributed,checkpoint}.rs, type/input selector seams, tests/network_gateway_frer.rs and examples. Root metadata/output/reference catalog, outer codec16 selection and Viewer terminal display were checked. Source hashes are retained in gateway-frer-review-source-hashes.json. No product file was changed.

# Interfaces / correctness findings

- GatewaySourceEvidence reconstructs bytes/FCS, VID/PCP, conversion/branch/child IDs and lineage from actual successful CAN request/receiver evidence and the prepared rule. Processing/offer timing and admitted child binding are checked. Generator sources and Gateway sources remain distinct; selector conflicts and invalid/nonordinary prefixes are rejected.
- Actual ordinary-prefix SOF is the source notification. Prefix acceptance drives the existing FRER sequence/branch path. Queued conversion/backpressure cannot create a fictitious source SOF or a future terminal completion.
- Terminal indication is emitted only from an actually received suffix at an endpoint, linked to the accepted recovery round. Later timeout-accepted rounds have distinct real reception identities; the Gateway unique-target summary preserves earliest completion. Circuit transfer IDs distinguish visits/copies/rounds.
- Restore compares the prepared selector, actual CAN/conversion proof, injected frame data, prefix transfer/SOF, distributed flow and actual suffix reception ledger. It checks completeness, uniqueness, round acceptance and earliest target summary, rather than accepting a separately asserted Gateway narrative. Existing FRER and MAC checkpoint owners continue validating their domains.
- Injection and branch preparation remain staged. Reserve failures at the two real callback cuts preserve the entire private network state and the adapter's FES/journal/effect contract. The failpoints are reached through saved actual callbacks, not fabricated callback timing.
- Private and outer owned-state codec16 are selected by has_gateway_replication; absent Gateway provenance preserves earlier version selection. Output metadata exposes source.kind=gateway_egress and removes the empty generator surrogate. The observed CLI result contains the real source selector and 501930000ps terminal indication; the Viewer table consumes actual terminal records. No browser rerun is claimed here.

# Acceptance-scope decision: 74B conversion and preemption

**Accept the honest split of evidence; do not add arbitrary padding or a new encapsulation/aggregation contract solely to satisfy the old literal multi-fragment wording.** This is an explicit refinement of the earlier design acceptance sentence, not a claim that its literal condition occurred.

IEEE 802.3-2022 99.3.5 specifies at least60 mData octets and no fragment pad field. 99.4.4 requires at least60 original octets already sent and at least64 original octets including FCS remaining. Thus the minimum original MAC frame that can be split at addFragSize0 is124B. The real conversion produces68B including VLAN/FCS, then an R-TAG branch74B. It cannot legally split. Its actual SMD-start/final-FCS packet traverses the real pMAC and is shaped by CBS/TAS; this correctly satisfies the converted traffic's physical path.

The same FES test adds a genuine native166-payload-byte background frame on the reverse branch path. That frame proves actual mCRC, suspension, checkpoint continuation and lost-final non-delivery, with an independent CRC oracle. It does not prove that the74B converted branch fragmented, nor that it was interrupted on the same forward serializer. Both real Gateway final pMAC packets being lost separately proves no false recovery/terminal for converted traffic. Preserve these distinct claims in docs/ledger/output descriptions.

A future requirement specifically demanding fragmented converted payload requires a separately approved larger wire-format or aggregation feature. It is not needed to finish the currently supported Gateway composition.

# Verification

Fresh independent runs of frozen existing binaries, no rebuild:

- evidence/bin/network-gateway-frer-17 --test-threads=1: **13 passed,0 failed**,31.13s; gateway-frer-review-independent13.log.
- evidence/bin/dir-simulator-gateway-lib06 --exact runtime::network::tests::gateway_real_conversion_and_replication_reserve_failures_are_atomic --nocapture: **1 passed,0 failed**,1.13s. This test exercises both real cuts; gateway-frer-review-independent-atomic.log.

Source assertions inspected: analytic CAN CRC15=11436/stuff2/110bits/220us EOF, actual conversion SOF226us, exact original suffix bytes,74B branches at500us, -564 credit bits, second genuine CAN request starting503.008us after CBS recovery, original CAN terminal501.93us. Seven fresh-process checkpoint cuts and13 rehashed mutations are included in the focused suite. The repeated-suffix-round test checks actual repeated indications without increasing the unique target count.

Producer logs independently read: focused13 and atomic1(two cuts); existing regressions total76 = frer_normative11 + frer_rtag10 + extensions11 + legacy_can3 + acceptance_bridge17 + mac_merge24. The handoff swapped extensions/legacy individual counts, but the total76 is correct. These regression suites were not rerun by this reviewer.

Commands: rg/sed source/normative inspection, Python JSON/hash extraction, and the two copied-binary invocations. No Cargo build, Git operation or publication. No claim that existing binaries were freshly rebuilt from this review's source hashes.

# Constraints / unresolved uncertainty

Ordinary non-Merge prefix, vector bridge-pair placement, external destination, static supported CBS/TAS and allowed link controls remain the declared bounds. No new cross-protocol scope is implied. Browser QA and wider suite are parent-owned. Coordinated rewriting of every physical/source root is outside bounded semantic checkpoint validation.

# Changed paths

Created this review, gateway-frer-review-source-hashes.json, gateway-frer-review-independent13.log and gateway-frer-review-independent-atomic.log under the evidence directory. Product source unchanged.

# Addendum: terminal Viewer causality found by parent browser QA

Parent browser QA subsequently found a P2 display defect outside this review's independently executed runtime scope: generic generated_ps birth exposed terminal indication rows before actual suffix reception (the future received_ps was hidden, but origin/end-to-end information leaked). The controlled model now assigns dir.can_ethernet.terminal_indication birth to received_ps. I inspected that branch and the new gateway_frer_viewer_model.test.cjs boundary test (499/500 and899/900, including backward seek). The supplied /tmp/dir-gateway-viewer-after-birth-fix.log reports **19 passed,0 failed** (not20). This is evidence of the model fix; browser regeneration/recheck remains parent-owned and pending at this addendum.

Accordingly, the runtime closure above remains valid within its stated bounds, while complete Viewer closure requires the parent's rebuilt-artifact browser result. No claim that this review independently ran browser QA is made.

# Parent post-review Viewer verification

The rebuilt CLI (evidence/bin/dir-simulator-named-4) embeds the received_ps birth fix. The actual Gateway result was rendered to gateway-frer-cli-current-1/viewer-after-birth-fix.html. /tmp/dir-composite-current-browser-3.log passes all four actual-result cases (T10 point-to-point, T10 repeater, delayed noncoordinator PLCA, Gateway composition). The Gateway terminal row is absent before actual suffix reception, present at completion with original CAN identity and501930000ps end-to-end time, and unchanged after backward/forward replay. Desktop1440 and mobile390 layouts have no horizontal overflow, page error or external request. The19-case Node gate is recorded in /tmp/dir-gateway-viewer-after-birth-fix.log; the earlier browser and unit failures remain retained.
