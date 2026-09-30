package gw;
network Chain {
 submodules:
  src: gw.Controller;
  g1: gw.Gateway;
  g2: gw.Gateway;
  g3: gw.Gateway;
  sink: gw.Controller;
  busA: gw.Bus;
  busB: gw.Bus;
  busC: gw.Bus;
  busD: gw.Bus;
 connections:
  src.tx --> gw.Wire --> busA.tx_a;
  busA.rx_a --> gw.Wire --> src.rx;
  g1.tx_a --> gw.Wire --> busA.tx_b;
  busA.rx_b --> gw.Wire --> g1.rx_a;
  g1.tx_b --> gw.Wire --> busB.tx_a;
  busB.rx_a --> gw.Wire --> g1.rx_b;
  g2.tx_a --> gw.Wire --> busB.tx_b;
  busB.rx_b --> gw.Wire --> g2.rx_a;
  g2.tx_b --> gw.Wire --> busC.tx_a;
  busC.rx_a --> gw.Wire --> g2.rx_b;
  g3.tx_a --> gw.Wire --> busC.tx_b;
  busC.rx_b --> gw.Wire --> g3.rx_a;
  g3.tx_b --> gw.Wire --> busD.tx_a;
  busD.rx_a --> gw.Wire --> g3.rx_b;
  sink.tx --> gw.Wire --> busD.tx_b;
  busD.rx_b --> gw.Wire --> sink.rx;
}
