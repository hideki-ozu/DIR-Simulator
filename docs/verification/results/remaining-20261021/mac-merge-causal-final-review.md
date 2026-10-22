# Goal

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`review-remaining-mac-merge-causal-20261021`
文書状態：未commit・未公開。選択範囲の独立レビュー記録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-22` | 履歴credit再計算とHOLD保持の最終独立レビュー |


Final independent correctness review of the frozen causal CBS checkpoint journal and retained HOLD lifecycle. **No new P1/P2 found within the stated bounded scope. The historical-credit N06 blocker and retained-HOLD lifecycle acceptance gap are closed by the inspected implementation and evidence.** This also preserves the prior bounded N04/N05 repair verdict; it does not expand those items into full IEEE conformance.

# Architecture

The existing NetworkState/NetworkDelta and common FES remain the execution engine. Journal replay creates TsnState::new from prepared configuration, starting at zero credit, and reconstructs inputs before comparing the entire serialized TSN port. It does not restore credit from a journal anchor, saved segment label, or cursor cache. Cursor equality is checked after reconstruction. Exact scaled signed credit, including the one-unit arithmetic residual, last-update time, mode, gate state and pending wake/generation are covered by that comparison.

# Files

Inspected runtime/network/mac_merge/{journal,checkpoint,integration,shaper,tas,tests}.rs, runtime/ethernet/{tsn.rs,tsn/merge.rs}, targeted network.rs::stage_tsn/refresh_tsn, tests/mac_merge.rs, the final handoff, prior P2 re-review, retained result evidence and test logs. All six entries in mac-merge-final-causal-source-hashes.json match current source bytes (journal, integration, tas, TSN merge helper, TSN owner, production tests). No product source was edited.

# Interfaces

1. Journal Start/Replan proofs retain physical bytes, source transfer ownership, fragment plan and fault outcome after the live packet ledger is pruned. Replay checks encoding against source MAC bytes, SOF/final EOF against source lifecycle, bit-duration/IPG/arrival timing, epoch/ordinal, active owner, and emitted-prefix preservation during replanning. Accounting classification is derived by packet_view from those proofs; it is not accepted as an arbitrary segment assertion.
2. Queue offers are bound to original queued_ps and direction. FIFO selection checks prepared class masks plus CBS/TAS eligibility. Selected pMAC remains backlog through express interruption and can only unselect after a final physical release. Reset causes are bound to prepared link controls. Physical boundaries cannot be skipped across replay observations.
3. Chronological entry indexes/times and cause ranks enforce the recorded transition order, including EOF before release, release before unselect, enqueue before select, and selection before physical start. Static gate timing derives from prepared schedules; open_elapsed independently integrates gate-open intervals. Reordering saved credit/mode/backlog/wake values together cannot bypass the final equality against replayed state while these causal roots remain fixed.
4. Staging appends the journal before committing the TSN delta. Event/octet limits reject further history explicitly; there is no silent pruning or arbitrary compaction anchor. The allocation failpoint source covers journal append, retained packet proof and queue cause alongside the original five points. Its oracle compares the exact committed TSN and MAC prefix. The additive plan_control_in consumes the staged port rather than losing an earlier same-callback MAC transition.
5. HOLD is a latch independent of current pActive. merge_tas_event changes it only for an actual active operation; inactive operations are recorded but do not update applied_tas. Arbitration allows Verify/Respond while a retained pMAC is held, enabling reverification without bypassing HOLD. Subsequent active RELEASE resumes the original selected frame. Checkpoint validation rejects a standalone HOLD flip.

Normative HOLD inspection used the supplied licensed local IEEE 802.3-2022 text, 99.2.1.3 and 99.4.7 (hold/pActive/pAllow), and IEEE 802.1Q-2022 Table 8-7 and 12.30.1.5. These support retaining the latch through capability loss and treating inactive new gate operations as gate-state-only operations. No automatic RELEASE on capability loss is implied.

# Constraints

Static ideal-clock CBS/TAS is the supported composition. GCL replacement and local-clock CBS/TAS remain guarded product limitations and are not completed by this review. This establishes causal consistency with retained source/physical evidence, not cryptographic authentication against coordinated rewriting of every source root and history. No broad IEEE certification, outer codec review, broad suite or browser rerun is claimed. No network_ieee_t1s rebuild was performed.

# Acceptance criteria

Closed within scope: fresh-zero historical replay, exact credit/residual comparison, retained completed packet evidence, source/FIFO/backlog binding, physical overhead transitions, deterministic causal order, explicit bounded-history failure, and actual HOLD -> matching shutdown -> inactive RELEASE ignored -> reverification while held -> real active RELEASE -> original continuation completion. Prior repaired LLDP withdrawal and incomplete-RX checkpoint cases remain covered by the production suite.

# Verification

Fresh independent execution, using the already-built binary without rebuilding:

`/tmp/dir-remaining-implementation-target/debug/deps/mac_merge-82c4ce1cb7ff2d6d --test-threads=1`

Result: **24 passed, 0 failed**, 60.07 seconds. Log: `mac-merge-causal-final-review-independent-tests.log`. This includes nine historical credit/causal mutants, the 16-position retained-HOLD lifecycle, link-epoch proof restoration, CBS/TAS resumes, both bounded-history limits, LLDP withdrawal and RX checkpoint regressions. Test source was inspected for its actual assertions; no new mutant was added. Because this was an existing binary, it is independent execution evidence rather than a fresh source-to-binary rebuild claim.

Separately inspected producer evidence: core 19/19 (including eight reservation failpoints), production 24/24 and Viewer 1/1. Those core/Viewer executions were not independently rerun. Reviewed source hashes and retained lifecycle JSON corroborate the frozen handoff.

# Findings and changed paths

No new P1/P2 finding. Only this review and its independent-test log were created under the evidence directory. Product source remains read-only.

# Unresolved uncertainty

No fresh compilation, exhaustive same-time event permutation search, coordinated all-root forgery campaign, broader suite or browser verification was performed. Full-history replay has explicit capacity limits; no performance or compaction claim is made. These limits do not reopen the specific historical-credit or HOLD defects reviewed here.
