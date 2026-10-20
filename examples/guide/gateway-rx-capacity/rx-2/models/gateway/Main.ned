package gateway;
network Main {
 submodules:
  src: gateway.Controller;
  gw: gateway.Fanout;
  sinkB: gateway.Controller;
  sinkC: gateway.Controller;
  busA: gateway.Bus;
  busB: gateway.Bus;
  busC: gateway.Bus;
 connections:
  src.tx --> gateway.Wire --> busA.rx_a;
  busA.tx_a --> gateway.Wire --> src.rx;
  gw.tx_a --> gateway.Wire --> busA.rx_b;
  busA.tx_b --> gateway.Wire --> gw.rx_a;
  gw.tx_b --> gateway.Wire --> busB.rx_a;
  busB.tx_a --> gateway.Wire --> gw.rx_b;
  sinkB.tx --> gateway.Wire --> busB.rx_b;
  busB.tx_b --> gateway.Wire --> sinkB.rx;
  gw.tx_c --> gateway.Wire --> busC.rx_a;
  busC.tx_a --> gateway.Wire --> gw.rx_c;
  sinkC.tx --> gateway.Wire --> busC.rx_b;
  busC.tx_b --> gateway.Wire --> sinkC.rx;
}
