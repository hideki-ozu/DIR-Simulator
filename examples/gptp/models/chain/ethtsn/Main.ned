package ethtsn;
simple Endpoint {
 parameters:
  @class("dir.ethernet.Endpoint");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
 gates: input rx; output tx;
}
channel Wire {
 parameters:
  @class("dir.ethernet.Link");
  double bitrate @unit(bps) = default(1Gbps);
  double delay @unit(s) = default(100ns);
}
simple Bridge {
 parameters:
  @class("dir.ethernet.Switch");
  int queueCapacity = default(64);
 gates:
  input rx_a; output tx_a;
  input rx_c; output tx_c;
}
network Net {
 submodules:
  a: ethtsn.Endpoint;
  b: ethtsn.Bridge;
  c: ethtsn.Endpoint;
 connections:
  a.tx --> ethtsn.Wire --> b.rx_a;
  b.tx_a --> ethtsn.Wire --> a.rx;
  b.tx_c --> ethtsn.Wire --> c.rx;
  c.tx --> ethtsn.Wire --> b.rx_c;
}
