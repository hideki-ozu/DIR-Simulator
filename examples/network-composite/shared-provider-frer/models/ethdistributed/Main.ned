package ethdistributed;
simple Endpoint {
 parameters: @class("dir.ethernet.Endpoint"); int queueCapacity = default(64); double txProcessingDelay @unit(s) = default(0ps); double rxProcessingDelay @unit(s) = default(3000ps);
 gates: output tx; input rx;
}
simple R {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: output tx_s; input rx_s; output tx_a; input rx_a; output tx_b; input rx_b;
}
simple A {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: output tx_r; input rx_r; output tx_e; input rx_e;
}
simple B {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_x; output tx_x; output tx_r; input rx_r; output tx_e; input rx_e;
}
simple E {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: output tx_a; input rx_a; output tx_b; input rx_b; output tx_d; input rx_d;
}
channel Wire { parameters: @class("dir.ethernet.Link"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(1000ps); }
simple H { parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64); gates: input rx_r; output tx_r; input rx_a; output tx_a; input rx_x; output tx_x; }
network Net {
 submodules: s: ethdistributed.Endpoint; d: ethdistributed.Endpoint; r: ethdistributed.R; a: ethdistributed.A; b: ethdistributed.B; e: ethdistributed.E; h: ethdistributed.H;
 connections:
  s.tx --> ethdistributed.Wire --> r.rx_s; r.tx_s --> ethdistributed.Wire --> s.rx;
  r.tx_a --> ethdistributed.Wire --> h.rx_r; h.tx_r --> ethdistributed.Wire --> r.rx_a;
 a.tx_r --> ethdistributed.Wire --> h.rx_a; h.tx_a --> ethdistributed.Wire --> a.rx_r;
 b.tx_x --> ethdistributed.Wire --> h.rx_x; h.tx_x --> ethdistributed.Wire --> b.rx_x;
  r.tx_b --> ethdistributed.Wire --> b.rx_r; b.tx_r --> ethdistributed.Wire --> r.rx_b;
  a.tx_e --> ethdistributed.Wire --> e.rx_a; e.tx_a --> ethdistributed.Wire --> a.rx_e;
  b.tx_e --> ethdistributed.Wire --> e.rx_b; e.tx_b --> ethdistributed.Wire --> b.rx_e;
  e.tx_d --> ethdistributed.Wire --> d.rx; d.tx --> ethdistributed.Wire --> e.rx_d;
}
