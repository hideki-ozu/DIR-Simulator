# Goal

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`review-remaining-bridge-expanded-20261021`
文書状態：未commit・未公開。選択範囲の独立レビュー記録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-22` | 拡張gateの独立レビューと統合再検証の追記 |

Scoped N09/N10/N11 expanded bridge review. No additional blocking P1/P2 identified in inspected changes; closure remains bounded to selected adapters and evidence below. Integrated MST boundary rerun is pending, not passed.

# Architecture
Real control/data packets continue through one physical FES. Per-port topology-change flushing, CIST/MSTI selection, provider customer/service-domain transport and internal ISS forwarding remain separate state owners with typed attachment contracts.

# Files
Read prior shared-rstp-flush-review.md; current runtime/network/wire.rs, runtime/wire/mstp.rs, tests/network_shared_tree.rs and network_mstp_boundary.rs; provider topology/mutant/browser logs and current MST rerun log. No product changes or builds.

# Interfaces
N09: revised fixture's actual b-to-a flush_probe selects the exact learned s1.tx_b victim. The test now binds source B to that port before its 36s loss and checks its removal. This implements prior Q13.27.28 port-scoped diagnosis; source A on unaffected s2 is not a justified mandatory victim.

N10: other_ports_synced exempts operEdge candidates in both CIST and MSTI call paths. This repairs the selected handshake obstruction without changing packet-version semantics. Four production fixture variants explicitly mutate name, revision, VLAN map, or actual peer protocol. Assertions follow actual physical parent chains, exact MAC bytes and destination reception; non-RSTP variants check observed boundaries and Master, while RSTP case checks v2 transmission, v3 transmission and successful CIST receive. Q13.7.1 allows an MST bridge to continue version3 with an RSTP peer; no auto-v2 requirement is imposed. This helper is a bounded synchronization model, not evidence that all Q allSynced state-machine predicates are implemented.

N11: validate_transport binds component owner, physical egress, envelope domain, service VID/tree, static registrar membership and source-byte constraints for local control. Relayed customer traffic must belong to provider transport and its S-tag/tree mapping. It runs on restored TX objects. Forward disposition requires ingress tree forwarding/link state (and learning, consistent with forwarding state), and relayed logical egress is rechecked against current tree forwarding. These prevent the inspected standalone transport-domain forgery and forwarding-while-learning-only path. This is not authentication of coherently rewritten historical relay sources.

# Constraints
No full IEEE certification or blanket historical provenance claim. Remaining unrelated PHY/gPTP edits excluded. No Git/publication or rebuild contention. Multi-PEP multiplicity and topology-flush tests establish selected scenarios, not arbitrary topology coverage.

# Acceptance criteria
N09 corrected scoped fixture approved. N10 four-variant test design and edge repair approved, but final coherent current-source production rerun required. N11 scoped closure supported by inspected implementation plus current producer runs. No newly proven causal defect in these edits.

# Verification
Provider actual unexpired S100/S200 topology-flush test:1 passed. Rehashed transport-domain mutant test:1 passed (six mutations as parent reports). Browser log: both forwarding-control and multi-PEP actual component/cursor desktop/mobile checks passed. Parent reports multi-PEP8 deliveries/8PNP/26copies and core MST18/base1; these are producer results, not fresh independent executions. /tmp/dir-mstp-expanded-production-current.log currently ends in missing t1s_chips/t1s_group initializer compilation failure in concurrent PHY work, so it proves no integrated MST result. Uncertainty is this rerun and absence of independent executions, not a discovered bridge logic failure.

## 統合担当によるレビュー後の再検証

上記レビューで残っていたcurrent-source gateを再実行した。`cargo test -p dir-simulator --test network_mstp_boundary --test network_mstp -- --nocapture`が成功し、基本製品1件（32.15s）とname/revision/map/RSTP peerの4構成を含む境界製品1件（101.70s）が通った。ログは`/tmp/dir-mstp-expanded-production-current-2.log`へ保存した。前の並行編集時のcompile失敗ログは保持している。これは統合担当の再実行結果であり、独立レビュアー自身による再実行とは区別する。
