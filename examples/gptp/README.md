# gPTP packet-path examples

Run each `.ini` from the repository root with `dir-simulator run --config examples/gptp/<name>.ini --output <empty-directory>`.
The selected profile is a bounded IEEE 802.1AS-2020 domain-0, two-step, full-duplex physical tree. The two-node model uses independent raw oscillators and a 100 ns link. The chain models use Net.a as the initial grandmaster, Net.b as a time-aware relay, and Net.c as the downstream node. Results include actual Ethernet transfers, RX/TX timestamp captures, decoded gPTP records, and applied clock changes.

| Config | Evidence |
| --- | --- |
| `two-node.ini` | Peer delay, Announce selection, and Net.b correction. |
| `two-node-merge.ini` | The same messages traverse the express MAC Merge packet path. |
| `two-node-servo-enabled.ini`, `two-node-servo-disabled.ini` | Same data workload with or without packet discipline; TAS departure and PSFP reception differ. |
| `two-node-drop-sync.ini`, `two-node-drop-followup.ini`, `two-node-bad-fcs-followup.ini` | Physical MAC packet faults prevent incomplete or corrupt first Sync exchange from correcting the clock. |
| `two-node-delayed-followup.ini` | A 10 Mbps same-priority data frame occupies the transmitter after Sync, so its Follow_Up arrives after the pairing deadline and is rejected. |
| `three-node.ini` | Net.b relay correction and Net.c correction. |
| `three-node-queue.ini` | A 1,000-byte data frame occupies Net.b's egress; the relay Sync waits and its Follow_Up correction grows. |
| `three-node-psfp-open.ini`, `three-node-psfp-closed.ini` | PSFP admits or drops that data frame, changing the relay Sync departure. |
| `three-node-tas.ini` | A static TAS gate delays the relay Sync. |
| `three-node-failover.ini`, `three-node-recovery.ini` | Loss of the Net.a–Net.b link elects Net.b; recovery restores Net.a. |

The packet and checkpoint behavior is exercised by `cargo test -p dir-simulator --test network_gptp`.
