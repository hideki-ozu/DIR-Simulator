package ethaccept;
simple Endpoint {
 parameters: @class("dir.ethernet.Endpoint"); int queueCapacity = default(64); double txProcessingDelay @unit(s) = default(0ps); double rxProcessingDelay @unit(s) = default(0ps);
 gates: input rx; output tx;
}
simple Switch {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_a; output tx_a; input rx_b; output tx_b; input rx_c; output tx_c; input rx_d; output tx_d;
}
channel Wire { parameters: @class("dir.ethernet.Link"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(0ps); }
channel Slow { parameters: @class("dir.ethernet.Link"); double bitrate @unit(bps) = default(10Mbps); double delay @unit(s) = default(0ps); }
network Net {
 submodules: a: ethaccept.Endpoint; b: ethaccept.Endpoint; c: ethaccept.Endpoint; d: ethaccept.Endpoint; s: ethaccept.Switch;
 connections:
 a.tx --> ethaccept.Wire --> s.rx_a; s.tx_a --> ethaccept.Wire --> a.rx;
 b.tx --> ethaccept.Slow --> s.rx_b; s.tx_b --> ethaccept.Slow --> b.rx;
 c.tx --> ethaccept.Wire --> s.rx_c; s.tx_c --> ethaccept.Wire --> c.rx;
 d.tx --> ethaccept.Wire --> s.rx_d; s.tx_d --> ethaccept.Wire --> d.rx;
}
