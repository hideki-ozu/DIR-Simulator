package gatewayfrer;
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
 gates: output tx_r; input rx_r; output tx_e; input rx_e;
}
simple E {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: output tx_a; input rx_a; output tx_b; input rx_b; output tx_d; input rx_d;
}
channel Wire { parameters: @class("dir.ethernet.Link"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(1000ps); }
simple CanController {
 parameters: @class("dir.bridge.CanController"); int queueCapacity = default(64); double txProcessingDelay @unit(s) = default(0ps); double rxProcessingDelay @unit(s) = default(0ps); string rxFilter = default("*");
 gates: input rx; output tx;
}
simple CanBus {
 parameters: @class("dir.bridge.CanBus"); double bitrate @unit(bps) = default(500kbps);
 gates: input rx_a; input rx_b; output tx_a; output tx_b;
}
module Gateway {
 gates: input can_rx; output can_tx; input eth_rx; output eth_tx;
 submodules: can: gatewayfrer.CanController; eth: gatewayfrer.Endpoint;
 connections: can_rx --> can.rx; can.tx --> can_tx; eth_rx --> eth.rx; eth.tx --> eth_tx;
}
network Net {
 submodules: gw: gatewayfrer.Gateway; src: gatewayfrer.CanController; bus: gatewayfrer.CanBus; d: gatewayfrer.Endpoint; r: gatewayfrer.R; a: gatewayfrer.A; b: gatewayfrer.B; e: gatewayfrer.E;
 connections:
  src.tx --> bus.rx_a; bus.tx_a --> src.rx; gw.can_tx --> bus.rx_b; bus.tx_b --> gw.can_rx;
  gw.eth_tx --> gatewayfrer.Wire --> r.rx_s; r.tx_s --> gatewayfrer.Wire --> gw.eth_rx;
  r.tx_a --> gatewayfrer.Wire --> a.rx_r; a.tx_r --> gatewayfrer.Wire --> r.rx_a;
  r.tx_b --> gatewayfrer.Wire --> b.rx_r; b.tx_r --> gatewayfrer.Wire --> r.rx_b;
  a.tx_e --> gatewayfrer.Wire --> e.rx_a; e.tx_a --> gatewayfrer.Wire --> a.rx_e;
  b.tx_e --> gatewayfrer.Wire --> e.rx_b; e.tx_b --> gatewayfrer.Wire --> b.rx_e;
  e.tx_d --> gatewayfrer.Wire --> d.rx; d.tx --> gatewayfrer.Wire --> e.rx_d;
}
