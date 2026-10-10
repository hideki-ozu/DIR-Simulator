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
 gates: input rx_a; input rx_b; output tx_a; output tx_b;
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
  double delay @unit(s) = default(4000001ps);
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
network R01 {
 submodules:
  src: bridge.CanController;
  bus: bridge.CanBus;
  gw: bridge.Gateway01;
  sink: bridge.EthEndpoint;
 connections:
  src.tx --> bus.rx_a; bus.tx_a --> src.rx;
  gw.can_tx --> bus.rx_b; bus.tx_b --> gw.can_rx;
  gw.eth_tx --> bridge.Link01 --> sink.rx;
  sink.tx --> bridge.Link01 --> gw.eth_rx;
}
network R02 {
 submodules:
  src: bridge.CanController;
  bus: bridge.CanBus;
  gw: bridge.Gateway02;
  sink: bridge.EthEndpoint;
 connections:
  src.tx --> bus.rx_a; bus.tx_a --> src.rx;
  gw.can_tx --> bus.rx_b; bus.tx_b --> gw.can_rx;
  gw.eth_tx --> bridge.Link02 --> sink.rx;
  sink.tx --> bridge.Link02 --> gw.eth_rx;
}
module GatewayFanout {
 gates: input can_a_rx; output can_a_tx; input can_b_rx; output can_b_tx; input eth_rx; output eth_tx;
 submodules: can_a: bridge.CanController; can_b: bridge.CanController; eth: bridge.EthEndpoint;
 connections:
  can_a_rx --> can_a.rx; can_a.tx --> can_a_tx;
  can_b_rx --> can_b.rx; can_b.tx --> can_b_tx;
  eth_rx --> eth.rx; eth.tx --> eth_tx;
}
network Fanout {
 submodules:
  src_a: bridge.CanController; src_b: bridge.CanController;
  bus_a: bridge.CanBus; bus_b: bridge.CanBus;
  gw: bridge.GatewayFanout; sink: bridge.EthEndpoint;
 connections:
  src_a.tx --> bus_a.rx_a; bus_a.tx_a --> src_a.rx;
  gw.can_a_tx --> bus_a.rx_b; bus_a.tx_b --> gw.can_a_rx;
  src_b.tx --> bus_b.rx_a; bus_b.tx_a --> src_b.rx;
  gw.can_b_tx --> bus_b.rx_b; bus_b.tx_b --> gw.can_b_rx;
  gw.eth_tx --> bridge.Link02 --> sink.rx;
  sink.tx --> bridge.Link02 --> gw.eth_rx;
}
