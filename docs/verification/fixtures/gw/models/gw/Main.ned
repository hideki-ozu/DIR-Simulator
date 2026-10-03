package gw;
network Main {
 submodules:
  src: gw.Controller;
  gw: gw.Gateway;
  sink: gw.Controller;
  busA: gw.Bus;
  busB: gw.Bus;
 connections:
  src.tx --> gw.Wire --> busA.rx_a;
  busA.tx_a --> gw.Wire --> src.rx;
  gw.tx_a --> gw.Wire --> busA.rx_b;
  busA.tx_b --> gw.Wire --> gw.rx_a;
  gw.tx_b --> gw.Wire --> busB.rx_a;
  busB.tx_a --> gw.Wire --> gw.rx_b;
  sink.tx --> gw.Wire --> busB.rx_b;
  busB.tx_b --> gw.Wire --> sink.rx;
}
