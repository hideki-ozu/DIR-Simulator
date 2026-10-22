# Goal

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`review-remaining-frer-20261021`
文書状態：未commit・未公開。選択範囲の独立レビュー記録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 選択契約の実装・検証と独立レビューの記録 |
Scoped N01/N02 vector recovery and checkpoint-cut closure: **the final exact-cut P2 is closed; no new P1/P2 established in this bounded review.**

# Architecture
Selected vector_8021cb_2017 runs within the existing NetworkState transaction, using one phase-zero network Control and independent per-stream recovery state. The Control consumes all existing due items at its timestamp. Accepted recovery schedules a strictly future regular tick; other deliveries use phase one and arbitration phase two. Therefore an active recovery deadline at or before the last committed timestamp is impossible in this owner model.

# Files
Read frer-normative-contract.md; runtime/network/{replication,distributed,frer_wire,checkpoint}.rs; network.rs Control dispatch; registered.rs restore call; tests/frer_normative.rs; output/network/validation.rs; Viewer evidence logs. Primary comparison used the authorized local IEEE-802.1CB-2017.txt, especially sections 7.4.3 and 7.8. Product files were not changed.

# Interfaces
Vector recovery uses signed modulo-65536 distance, rejects both +/-history boundaries and half-range as rogue, refreshes only accepted inputs, and resets history through actual timeout processing. The legacy algorithm remains a separate selection. R-TAG decoding uses actual bytes/FCS and ignores reserved receive bits. Endpoint acceptance emits repeated physical delivery indications; bridge acceptance creates independent suffix rounds, including while earlier rounds remain in flight or after failure. Output validation binds counts and indications to reception and transfer evidence.

Replay derives recovery decisions from physical evidence and compares timer/reset history; timer-owner checks require exact active due generation/deadline. The added registered restore check now rejects any RecoveryTimeout due.at <= last_committed_key.time. It no longer treats a newly invented same-time higher-sequence phase-zero reservation as permission to undo a committed timeout. This reasoning depends on the documented sole-Control invariant; changes introducing additional independent phase-zero deliveries must reconsider the guard.

# Constraints
Selected nonindividual vector recovery, configured stream association, supported R-TAG placement, and generic project diagnostics only. No whole-device IEEE conformance, all managed counters, arbitrary coordinated whole-history forgery resistance, or new graphical browser QA is claimed. Viewer log inspection and output validation are separate from live browser rendering.

# Acceptance criteria
The strengthened regression restores a pre-timeout stream state, removes reset evidence, and forges a live same-time Control after an actually committed timeout in both delta1 and delta0/higher-sequence forms. Both are now rejected. Baseline fresh-process restart continues to pass before expiry, after first completion, at timeout, and during the second continuation, plus endpoint restart. Production assertions establish two actual deliveries, failed-first/successful-second suffix behavior, overlapping suffixes, and timeout-before-reception at an equal timestamp.

# Verification
Independently inspected /tmp/dir-frer-exact-cut-final.log: all 11 frer_normative integration tests passed. Earlier /tmp/dir-frer-exact-cut-before.log and /tmp/dir-frer-exact-cut-sequence-before.log preserve the respective pre-fix failures. Supplied frer-core-final2.log and frer-wire-final.log each show 5/5 tests passed; frer-legacy-final.log shows 10/10. frer-viewer-production.log reports 14 actual production artifacts checked, and frer-public-viewer-check.log records progression to two accepted/two completed deliveries. No new Cargo tests or browser executions were run by this reviewer.
