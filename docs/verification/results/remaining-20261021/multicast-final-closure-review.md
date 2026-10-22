# Goal

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`review-remaining-multicast-20261021`
文書状態：未commit・未公開。選択範囲の独立レビュー記録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 選択契約の実装・検証と独立レビューの記録 |
Scoped N07/N08 closure: **both final TO_IN compatibility P2 findings are closed; no new P1/P2 established in this bounded review.**

# Architecture
Actual control packets feed the existing active multicast actor; production transmissions use the network control queue and actor membership feeds forwarding. Checkpoint restoration reconstructs actor history and verifies its state.

# Files
Read multicast_active.rs router and snooper compatibility branches and regressions, multicast_integration.rs replay, prior multicast-rfc-final/closure/3p2 reviews, and local primary RFC 9776 and RFC 9777 text. Product files were not changed.

# Interfaces
Router TO_IN suppression now requires IPv4 with a live IGMPv1 member timer. IGMPv2 and MLDv1 compatibility retain TO_IN processing. Snooping now records a distinct older_v1_until only for IGMPv1 reports, so an IGMPv2 listener does not suppress TO_IN. BLOCK suppression and TO_EX empty-source translation remain. These distinctions match RFC 9776 section 7.3.2 and RFC 9777 section 8.3.2.

# Constraints
This is a selected behavior review, not full RFC certification. Snooper tests directly establish source-lease retention; production tests establish the broader actor-to-physical-forwarding path. Query-loss evidence shows actual loss followed by pruning, not additional pruning caused by loss. No arbitrary coordinated ledger-forgery resistance claim.

# Acceptance criteria
The new router regression checks observable membership after the original legacy group lease for IGMPv1 (false), IGMPv2 (true), MLDv1 (true). The snooper regression checks a retained, unexpired source after the IGMPv2 group lease expires. These tests discriminate the previously wrong suppression rules. Earlier pending-change retransmission and MLDv1 advertised-delay repairs remain covered in the final actor suite. Same-time Change/Timer restoration retains the reserved wake before applying Change.

# Verification
Independently inspected supplied logs: multicast-rfc-repair/actor-tests-toin-final.log has 25/25 actor tests passed; /tmp/dir-mc-provider-coherent-gates.log has 2/2 active-multicast production tests passed; /tmp/dir-mc-checkpoint-final.log has 4/4 multicast checkpoint tests passed, including separate-process continuation, scheduled-change/same-time timer, and both rehashed membership mutants. No new tests run by this reviewer.
