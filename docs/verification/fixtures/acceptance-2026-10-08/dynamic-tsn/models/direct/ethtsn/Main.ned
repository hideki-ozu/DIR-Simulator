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
  double delay @unit(s) = default(0ps);
}
network Net {
 submodules: a: ethtsn.Endpoint; b: ethtsn.Endpoint;
 connections:
  a.tx --> ethtsn.Wire --> b.rx;
  b.tx --> ethtsn.Wire --> a.rx;
}
