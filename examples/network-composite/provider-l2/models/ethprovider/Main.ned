package ethprovider;
simple Endpoint {
 parameters: @class("dir.ethernet.Endpoint"); int queueCapacity = default(64); double txProcessingDelay @unit(s) = default(0ps); double rxProcessingDelay @unit(s) = default(0ps);
 gates: output tx; input rx;
}
simple East {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_a0; output tx_a0; input rx_a1; output tx_a1; input rx_a2; output tx_a2; input rx_b0; output tx_b0; input rx_pnp; output tx_pnp;
}
simple West {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_a3; output tx_a3; input rx_b1; output tx_b1; input rx_pnp; output tx_pnp;
}
channel Wire { parameters: @class("dir.ethernet.Link"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(1000ps); }
network Net {
 submodules:
  a0: ethprovider.Endpoint; a1: ethprovider.Endpoint; a2: ethprovider.Endpoint; a3: ethprovider.Endpoint;
  b0: ethprovider.Endpoint; b1: ethprovider.Endpoint;
  east: ethprovider.East; west: ethprovider.West;
 connections:
  a0.tx --> ethprovider.Wire --> east.rx_a0; east.tx_a0 --> ethprovider.Wire --> a0.rx;
  a1.tx --> ethprovider.Wire --> east.rx_a1; east.tx_a1 --> ethprovider.Wire --> a1.rx;
  a2.tx --> ethprovider.Wire --> east.rx_a2; east.tx_a2 --> ethprovider.Wire --> a2.rx;
  b0.tx --> ethprovider.Wire --> east.rx_b0; east.tx_b0 --> ethprovider.Wire --> b0.rx;
  a3.tx --> ethprovider.Wire --> west.rx_a3; west.tx_a3 --> ethprovider.Wire --> a3.rx;
  b1.tx --> ethprovider.Wire --> west.rx_b1; west.tx_b1 --> ethprovider.Wire --> b1.rx;
  east.tx_pnp --> ethprovider.Wire --> west.rx_pnp; west.tx_pnp --> ethprovider.Wire --> east.rx_pnp;
}
