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
  input rx_a; output tx_a;
  input rx_b; output tx_b;
  input rx_c; output tx_c;
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
  a.tx --> demo.Wire --> bus.rx_a;
  bus.tx_a --> demo.Wire --> a.rx;
  b.tx --> demo.Wire --> bus.rx_b;
  bus.tx_b --> demo.Wire --> b.rx;
  c.tx --> demo.Wire --> bus.rx_c;
  bus.tx_c --> demo.Wire --> c.rx;
}
