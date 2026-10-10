network Main {
 submodules:
  a: __PACKAGE__.MultibusController;
  b: __PACKAGE__.MultibusController;
  gw: __PACKAGE__.Gateway;
  busA: __PACKAGE__.MultibusBus;
  busB: __PACKAGE__.MultibusBus;
 connections:
  a.tx --> __PACKAGE__.FixedDelay --> busA.rx_a;
  busA.tx_a --> __PACKAGE__.FixedDelay --> a.rx;
  gw.tx_a --> __PACKAGE__.FixedDelay --> busA.rx_b;
  busA.tx_b --> __PACKAGE__.FixedDelay --> gw.rx_a;
  gw.tx_b --> __PACKAGE__.FixedDelay --> busB.rx_a;
  busB.tx_a --> __PACKAGE__.FixedDelay --> gw.rx_b;
  b.tx --> __PACKAGE__.FixedDelay --> busB.rx_b;
  busB.tx_b --> __PACKAGE__.FixedDelay --> b.rx;
}
