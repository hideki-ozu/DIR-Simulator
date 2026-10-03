package gw;
network Multi {
 submodules:
  src: gw.Controller;
  gw: gw.Fanout;
  sinkB: gw.Controller;
  sinkC: gw.Controller;
  busA: gw.Bus;
  busB: gw.Bus;
  busC: gw.Bus;
 connections:
  src.tx --> gw.Wire --> busA.rx_a;
  busA.tx_a --> gw.Wire --> src.rx;
  gw.tx_a --> gw.Wire --> busA.rx_b;
  busA.tx_b --> gw.Wire --> gw.rx_a;
  gw.tx_b --> gw.Wire --> busB.rx_a;
  busB.tx_a --> gw.Wire --> gw.rx_b;
  sinkB.tx --> gw.Wire --> busB.rx_b;
  busB.tx_b --> gw.Wire --> sinkB.rx;
  gw.tx_c --> gw.Wire --> busC.rx_a;
  busC.tx_a --> gw.Wire --> gw.rx_c;
  sinkC.tx --> gw.Wire --> busC.rx_b;
  busC.tx_b --> gw.Wire --> sinkC.rx;
}
