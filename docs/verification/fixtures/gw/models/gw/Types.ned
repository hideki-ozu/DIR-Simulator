package gw;
simple Controller {
 parameters:
  @class("dir.can.MultibusController");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
  string rxFilter = default("*");
 gates: output tx; input rx;
}
simple Bus {
 parameters:
  @class("dir.can.MultibusBus");
  double bitrate @unit(bps) = default(500kbps);
  string profile = default("can.cc.multibus.v1");
 gates: input tx_a; output rx_a; input tx_b; output rx_b;
}
channel Wire {
 parameters:
  @class("dir.link.FixedDelay");
  double delay @unit(s) = default(0ps);
}
module Gateway {
 gates:
  input rx_a; output tx_a;
  input rx_b; output tx_b;
 submodules:
  a: gw.Controller;
  b: gw.Controller;
 connections:
  rx_a --> a.rx;
  a.tx --> tx_a;
  rx_b --> b.rx;
  b.tx --> tx_b;
}
module Fanout {
 gates:
  input rx_a; output tx_a;
  input rx_b; output tx_b;
  input rx_c; output tx_c;
 submodules:
  a: gw.Controller;
  b: gw.Controller;
  c: gw.Controller;
 connections:
  rx_a --> a.rx;
  a.tx --> tx_a;
  rx_b --> b.rx;
  b.tx --> tx_b;
  rx_c --> c.rx;
  c.tx --> tx_c;
}
