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
simple CycleSwitch3 { parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64); gates: input rx_a; output tx_a; input rx_b; output tx_b; input rx_c; output tx_c; }
simple CycleSwitch2 { parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64); gates: input rx_a; output tx_a; input rx_b; output tx_b; }
network Cycle { submodules: src: bridge.CanController; bus: bridge.CanBus; gw: bridge.Gateway01; sink: bridge.EthEndpoint; s1: bridge.CycleSwitch3; s2: bridge.CycleSwitch3; s3: bridge.CycleSwitch2; connections:
 src.tx --> bus.rx_a; bus.tx_a --> src.rx;
 gw.can_tx --> bus.rx_b; bus.tx_b --> gw.can_rx;
 gw.eth_tx --> bridge.Link01 --> s1.rx_a; s1.tx_a --> bridge.Link01 --> gw.eth_rx;
 sink.tx --> bridge.Link01 --> s2.rx_a; s2.tx_a --> bridge.Link01 --> sink.rx;
 s1.tx_b --> bridge.Link01 --> s2.rx_b; s2.tx_b --> bridge.Link01 --> s1.rx_b;
 s1.tx_c --> bridge.Link01 --> s3.rx_a; s3.tx_a --> bridge.Link01 --> s1.rx_c;
 s2.tx_c --> bridge.Link01 --> s3.rx_b; s3.tx_b --> bridge.Link01 --> s2.rx_c;
}
