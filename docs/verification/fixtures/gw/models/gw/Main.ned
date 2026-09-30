package gw;
network Main {
 submodules:
  src: gw.Controller;
  gw: gw.Gateway;
  sink: gw.Controller;
  busA: gw.Bus;
  busB: gw.Bus;
 connections:
  src.tx --> gw.Wire --> busA.tx_a;
  busA.rx_a --> gw.Wire --> src.rx;
  gw.tx_a --> gw.Wire --> busA.tx_b;
  busA.rx_b --> gw.Wire --> gw.rx_a;
  gw.tx_b --> gw.Wire --> busB.tx_a;
  busB.rx_a --> gw.Wire --> gw.rx_b;
  sink.tx --> gw.Wire --> busB.tx_b;
  busB.rx_b --> gw.Wire --> sink.rx;
}
