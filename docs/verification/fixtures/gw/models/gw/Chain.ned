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
  src.tx --> gw.Wire --> busA.rx_a;
  busA.tx_a --> gw.Wire --> src.rx;
  g1.tx_a --> gw.Wire --> busA.rx_b;
  busA.tx_b --> gw.Wire --> g1.rx_a;
  g1.tx_b --> gw.Wire --> busB.rx_a;
  busB.tx_a --> gw.Wire --> g1.rx_b;
  g2.tx_a --> gw.Wire --> busB.rx_b;
  busB.tx_b --> gw.Wire --> g2.rx_a;
  g2.tx_b --> gw.Wire --> busC.rx_a;
  busC.tx_a --> gw.Wire --> g2.rx_b;
  g3.tx_a --> gw.Wire --> busC.rx_b;
  busC.tx_b --> gw.Wire --> g3.rx_a;
  g3.tx_b --> gw.Wire --> busD.rx_a;
  busD.tx_a --> gw.Wire --> g3.rx_b;
  sink.tx --> gw.Wire --> busD.rx_b;
  busD.tx_b --> gw.Wire --> sink.rx;
}
