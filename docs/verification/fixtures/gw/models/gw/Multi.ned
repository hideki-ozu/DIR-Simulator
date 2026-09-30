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
  src.tx --> gw.Wire --> busA.tx_a;
  busA.rx_a --> gw.Wire --> src.rx;
  gw.tx_a --> gw.Wire --> busA.tx_b;
  busA.rx_b --> gw.Wire --> gw.rx_a;
  gw.tx_b --> gw.Wire --> busB.tx_a;
  busB.rx_a --> gw.Wire --> gw.rx_b;
  sinkB.tx --> gw.Wire --> busB.tx_b;
  busB.rx_b --> gw.Wire --> sinkB.rx;
  gw.tx_c --> gw.Wire --> busC.tx_a;
  busC.rx_a --> gw.Wire --> gw.rx_c;
  sinkC.tx --> gw.Wire --> busC.tx_b;
  busC.rx_b --> gw.Wire --> sinkC.rx;
}
