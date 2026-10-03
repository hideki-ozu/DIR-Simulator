network Main {
 submodules:
  a: __PACKAGE__.Controller;
  b: __PACKAGE__.Controller;
  bus: __PACKAGE__.Bus;
 connections:
  a.tx --> __PACKAGE__.FixedDelay --> bus.rx_a;
  bus.tx_a --> __PACKAGE__.FixedDelay --> a.rx;
  b.tx --> __PACKAGE__.FixedDelay --> bus.rx_b;
  bus.tx_b --> __PACKAGE__.FixedDelay --> b.rx;
}
