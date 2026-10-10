module __NAME__ {
 gates:
  input rx_a;
  output tx_a;
  input rx_b;
  output tx_b;
 submodules:
  a: __CONTROLLER__;
  b: __CONTROLLER__;
 connections:
  rx_a --> a.rx;
  a.tx --> tx_a;
  rx_b --> b.rx;
  b.tx --> tx_b;
}
