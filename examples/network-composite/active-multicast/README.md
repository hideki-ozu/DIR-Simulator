# Active multicast composite example

`model.ini` exercises RFC 9776 IGMPv3 and RFC 9777 MLDv2 host/router actors on the existing Ethernet composite. `Net.b` joins IPv4 and IPv6 groups with source filters, adds another IPv4 source, then leaves. `Net.c` is a querier for both families. `workload.json` sends IPv4 and IPv6 multicast frames before and after those changes. The control packets use the network wire FIFO and switch snooping; the source filters apply to endpoint delivery.

Run with `dir-simulator run --config examples/network-composite/active-multicast/model.ini --output /tmp/active-multicast`. In `results.json`, `ipv4_other:0` is withheld from `Net.b` before the source is added, while `ipv4_other:1` is delivered afterward. `ipv4:3` and `ipv6:3` are withheld after the leave. The `ethernet.wire.membership` records show the reports accepted by the switches.
