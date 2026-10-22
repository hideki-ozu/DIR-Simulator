# Actual Gateway to bridge-pair FRER composition

Run `dir-simulator run --config model.ini --output <new-folder>` using the repository CLI syntax. This `can.ethernet.tsn.v1` fixture keeps one network event queue and the actual CAN, Gateway, Ethernet, TSN and Clause99 owners.

A real 500 kbit/s CAN frame (standard identifier 0x123, payload `0123456789abcdef`) enters `Net.gw.can`, matches `to_eth`, and becomes a DIRC Ethernet payload. The immutable replication selector is `{ "kind": "gateway_egress", "gateway_id": "Net.gw", "rule_id": "to_eth", "egress_index": 0 }`. The converted child has no Ethernet generator. It traverses ordinary `Net.gw.eth.tx`, replicates at `Net.r`, crosses separate branches through `Net.a` and `Net.b`, recovers at `Net.e`, and reaches the actual `Net.d` suffix endpoint.

Both branch outputs use ideal-clock TAS (priority3 closed for the first500us of every1ms cycle), CBS at250Mbit/s idle slope, and actual Clause99 capability advertisement/verification/serialization/reception. R-TAG insertion occurs at the replication bridge, after prefix reception and sequence allocation. The native converted frame is 68bytes before tagging,74bytes on the tagged branch, and traverses pMAC as one complete packet; it cannot legally be split into multiple Clause99 fragments.

`dir.can_ethernet.terminal_indication` records each real accepted suffix reception, including timeout reacceptance. `dir.can_ethernet.segment` retains the earliest completion for its one frozen destination. The original CAN origin and generation time remain in these records. Private/outer checkpoint16 is selected for Gateway replication; generator-only formats remain unchanged.

Validation evidence and any unresolved acceptance boundary are reported in the task evidence folder, not implied by the existence of this fixture. Full licensed standards are not included.

Clause99参加portのuntagged default priorityは7である。能力通知LLDPもこの分類を使い、開始時から開いているclass7で実際の能力交換とverificationを完了する。Gateway変換frameはVLAN PCP3を保持し、class3のTASが500usで開いた後にCBSの送信会計を受ける。
