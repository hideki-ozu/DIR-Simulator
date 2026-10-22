# Local Qcc management example

`model.ini` composes an identity-clock TSN network with the fully centralized local CUC/CNC adapter. The `high` and `low` unicast streams leave `Net.a` through the same managed output at different priorities. One Prepare operation requests both streams, participants accept by default after the declared management latency, and the schedule activates at 100,000,000 ps. Both generators produce frames after activation.

Run `dir-simulator run --config examples/network-composite/qcc/model.ini --output /tmp/qcc-example`. This example uses the local management profile `dir.cuc-cnc.local.v1`; it does not encode Qcc management messages on Ethernet.
