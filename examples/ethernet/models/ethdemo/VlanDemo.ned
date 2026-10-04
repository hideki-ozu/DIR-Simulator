package ethdemo;

// A connected tree used by the VLAN examples. Main.ned supplies Endpoint,
// Switch, and Wire; this file adds a three-switch trunk with access branches.
network VlanDemo {
 submodules:
  a: ethdemo.Endpoint;
  b: ethdemo.Endpoint;
  c: ethdemo.Endpoint;
  d: ethdemo.Endpoint;
  e: ethdemo.Endpoint;
  s1: ethdemo.Switch;
  s2: ethdemo.Switch;
  s3: ethdemo.Switch;
 connections:
  a.tx --> ethdemo.Wire --> s1.rx_a;
  s1.tx_a --> ethdemo.Wire --> a.rx;
  s1.tx_b --> ethdemo.Wire --> s2.rx_a;
  s2.tx_a --> ethdemo.Wire --> s1.rx_b;
  s1.tx_c --> ethdemo.Wire --> c.rx;
  c.tx --> ethdemo.Wire --> s1.rx_c;
  s2.tx_b --> ethdemo.Wire --> s3.rx_a;
  s3.tx_a --> ethdemo.Wire --> s2.rx_b;
  s2.tx_c --> ethdemo.Wire --> b.rx;
  b.tx --> ethdemo.Wire --> s2.rx_c;
  s3.tx_b --> ethdemo.Wire --> d.rx;
  d.tx --> ethdemo.Wire --> s3.rx_b;
  s3.tx_c --> ethdemo.Wire --> e.rx;
  e.tx --> ethdemo.Wire --> s3.rx_c;
}
