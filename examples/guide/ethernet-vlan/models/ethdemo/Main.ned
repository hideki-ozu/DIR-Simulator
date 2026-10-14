package ethdemo;
simple Endpoint {
 parameters:
  @class("dir.ethernet.Endpoint");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
 gates: output tx; input rx;
}
simple Switch {
 parameters:
  @class("dir.ethernet.Switch");
  int queueCapacity = default(64);
 gates:
  input rx_a; output tx_a;
  input rx_b; output tx_b;
  input rx_c; output tx_c;
}
channel Wire {
 parameters:
  @class("dir.ethernet.Link");
  double bitrate @unit(bps) = default(1Gbps);
  double delay @unit(s) = default(1000ps);
}
network Main {
 submodules:
  a: ethdemo.Endpoint;
  b: ethdemo.Endpoint;
  c: ethdemo.Endpoint;
  sw: ethdemo.Switch;
 connections:
  a.tx --> ethdemo.Wire --> sw.rx_a;
  sw.tx_a --> ethdemo.Wire --> a.rx;
  b.tx --> ethdemo.Wire --> sw.rx_b;
  sw.tx_b --> ethdemo.Wire --> b.rx;
  c.tx --> ethdemo.Wire --> sw.rx_c;
  sw.tx_c --> ethdemo.Wire --> c.rx;
}
