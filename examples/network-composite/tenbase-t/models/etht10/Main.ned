package etht10;
simple Endpoint {
 parameters: @class("dir.ethernet.Endpoint"); int queueCapacity = default(64); double txProcessingDelay @unit(s) = default(0ps); double rxProcessingDelay @unit(s) = default(0ps);
 gates: output tx; input rx;
}
simple Switch {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_a; output tx_a; input rx_b; output tx_b;
}
channel Wire { parameters: @class("dir.ethernet.Link"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(1000ps); }
channel Copper { parameters: @class("dir.ethernet.Link"); double bitrate @unit(bps) = default(10Mbps); double delay @unit(s) = default(1000ps); }
network Net {
 submodules: a: etht10.Endpoint; b: etht10.Endpoint; s1: etht10.Switch; s2: etht10.Switch;
 connections:
 a.tx --> etht10.Wire --> s1.rx_a; s1.tx_a --> etht10.Wire --> a.rx;
 b.tx --> etht10.Wire --> s2.rx_a; s2.tx_a --> etht10.Wire --> b.rx;
 s1.tx_b --> etht10.Copper --> s2.rx_b; s2.tx_b --> etht10.Copper --> s1.rx_b;
}
