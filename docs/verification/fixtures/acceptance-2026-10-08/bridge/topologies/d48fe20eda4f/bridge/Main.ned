package bridge;
simple CanController {
 parameters:
  @class("dir.bridge.CanController");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
  string rxFilter = default("*");
 gates: input rx; output tx;
}
simple CanBus {
 parameters: @class("dir.bridge.CanBus"); double bitrate @unit(bps) = default(500kbps);
 gates: input rx_a; input rx_b; input rx_c; input rx_d; output tx_a; output tx_b; output tx_c; output tx_d;
}
simple EthEndpoint {
 parameters:
  @class("dir.bridge.EthEndpoint");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
 gates: input rx; output tx;
}
channel Link01 {
 parameters: @class("dir.bridge.EthLink");
  double bitrate @unit(bps) = default(1Gbps);
  double delay @unit(s) = default(4us);
}
channel Link02 {
 parameters: @class("dir.bridge.EthLink");
  double bitrate @unit(bps) = default(1Gbps);
  double delay @unit(s) = default(1us);
}
module Gateway01 {
 gates: input can_rx; output can_tx; input eth_rx; output eth_tx;
 submodules:
  can: bridge.CanController;
  eth: bridge.EthEndpoint;
 connections:
  can_rx --> can.rx; can.tx --> can_tx;
  eth_rx --> eth.rx; eth.tx --> eth_tx;
}
module Gateway02 {
 gates: input can_rx; output can_tx; input eth_rx; output eth_tx;
 submodules:
  can: bridge.CanController;
  eth: bridge.EthEndpoint;
 connections:
  can_rx --> can.rx; can.tx --> can_tx;
  eth_rx --> eth.rx; eth.tx --> eth_tx;
}
simple CanBus2 { parameters: @class("dir.bridge.CanBus"); double bitrate @unit(bps) = default(500kbps); gates: input rx_a; input rx_b; output tx_a; output tx_b; }
simple Switch { parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64); gates: input rx_g1; input rx_g2; input rx_a; input rx_b; input rx_x; output tx_g1; output tx_g2; output tx_a; output tx_b; output tx_x; }
module GatewayDual { gates: input a_rx; output a_tx; input b_rx; output b_tx; input eth_rx; output eth_tx; submodules: a: bridge.CanController; b: bridge.CanController; eth: bridge.EthEndpoint; connections: a_rx --> a.rx; a.tx --> a_tx; b_rx --> b.rx; b.tx --> b_tx; eth_rx --> eth.rx; eth.tx --> eth_tx; }
network Loop { submodules: src: bridge.CanController; bus_a: bridge.CanBus2; bus_b: bridge.CanBus; g1: bridge.GatewayDual; g2: bridge.Gateway02; ca: bridge.CanController; cb: bridge.CanController; s: bridge.Switch; a: bridge.EthEndpoint; b: bridge.EthEndpoint; x: bridge.EthEndpoint; connections:
 src.tx --> bus_a.rx_a; bus_a.tx_a --> src.rx; g1.a_tx --> bus_a.rx_b; bus_a.tx_b --> g1.a_rx;
 g2.can_tx --> bus_b.rx_a; bus_b.tx_a --> g2.can_rx; g1.b_tx --> bus_b.rx_b; bus_b.tx_b --> g1.b_rx; ca.tx --> bus_b.rx_c; bus_b.tx_c --> ca.rx; cb.tx --> bus_b.rx_d; bus_b.tx_d --> cb.rx;
 g1.eth_tx --> bridge.Link02 --> s.rx_g1; s.tx_g1 --> bridge.Link02 --> g1.eth_rx;
 g2.eth_tx --> bridge.Link02 --> s.rx_g2; s.tx_g2 --> bridge.Link02 --> g2.eth_rx;
 a.tx --> bridge.Link02 --> s.rx_a; s.tx_a --> bridge.Link02 --> a.rx;
 b.tx --> bridge.Link02 --> s.rx_b; s.tx_b --> bridge.Link02 --> b.rx;
 x.tx --> bridge.Link02 --> s.rx_x; s.tx_x --> bridge.Link02 --> x.rx;
}
