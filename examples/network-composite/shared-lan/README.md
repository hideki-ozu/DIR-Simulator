# Three-bridge shared LAN

`model.ini` attaches `Net.s1.tx_b`, `Net.s2.tx_b`, and `Net.s3.tx_b` to one ordered MAC service. The `Net.hub` NED switch is a physical carrier placeholder; it does not execute a bridge protocol or receive data. A transmission occupies the segment through serialization and IFG release. At EOF, the same MAC frame is observed on the other two ingress ports after the configured propagation delay. The fixture sends two unicast frames from `Net.a` to `Net.b`; each shared transmission yields two physical receptions, and only the eligible destination branch delivers the frame.

Run with:

```sh
cargo run -p dir-simulator -- run --config examples/network-composite/shared-lan/model.ini --output /tmp/dir-shared-lan-example
```

The composite preparer currently supports one ordered shared service per declared segment, with equal carrier bitrate and delay. A sampled PHY, FRER replication, Qcc contract, and frame preemption cannot bind the same segment. This fixture demonstrates the data path; `wire_control` can also use the shared service for protocol frames.

`mstp.ini` uses the same carriers with one MSTP adapter on each bridge. It is a short initial-state run that shows control frames and data contend for the one segment reservation; MSTP's VLAN-to-instance map assigns VID 10 to MSTI 1 and VID 20 to MSTI 2. The live port state for each instance gates data on the corresponding VLAN.
