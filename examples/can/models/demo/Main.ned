package demo;
simple Controller {
 parameters:
  @class("dir.can.Controller");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
  string rxFilter = default("*");
 gates: output tx; input rx;
}
simple Bus {
 parameters:
  @class("dir.can.Bus");
  double bitrate @unit(bps);
  string profile = default("can.cc.ideal.v1");
 gates:
  input tx_a; output rx_a;
  input tx_b; output rx_b;
  input tx_c; output rx_c;
}
channel Wire {
 parameters:
  @class("dir.link.FixedDelay");
  double delay @unit(s) = default(0ps);
}
network Main {
 submodules:
  a: demo.Controller;
  b: demo.Controller;
  c: demo.Controller;
  bus: demo.Bus;
 connections:
  a.tx --> demo.Wire --> bus.tx_a;
  bus.rx_a --> demo.Wire --> a.rx;
  b.tx --> demo.Wire --> bus.tx_b;
  bus.rx_b --> demo.Wire --> b.rx;
  c.tx --> demo.Wire --> bus.tx_c;
  bus.rx_c --> demo.Wire --> c.rx;
}
