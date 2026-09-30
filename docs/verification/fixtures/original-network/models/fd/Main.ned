package fd;
simple Controller {
 parameters:
  @class("dir.canfd.ControllerV1");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
  string rxFilter = default("*");
 gates: output tx; input rx;
}
simple Bus {
 parameters:
  @class("dir.canfd.BusV1");
  double nominalBitrate @unit(bps);
  double dataBitrate @unit(bps);
  string profile = default("can.fd.precomputed.v1");
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
  a: fd.Controller;
  b: fd.Controller;
  c: fd.Controller;
  bus: fd.Bus;
 connections:
  a.tx --> fd.Wire --> bus.tx_a;
  bus.rx_a --> fd.Wire --> a.rx;
  b.tx --> fd.Wire --> bus.tx_b;
  bus.rx_b --> fd.Wire --> b.rx;
  c.tx --> fd.Wire --> bus.tx_c;
  bus.rx_c --> fd.Wire --> c.rx;
}
