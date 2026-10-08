package ethdynamic;
simple Endpoint {
 parameters:
  @class("dir.ethernet.Endpoint");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
 gates: output tx; input rx;
}
simple Switch {
 parameters:
  @class("dir.ethernet.Switch");
  int queueCapacity = default(64);
 gates:
  input rx_a; output tx_a;
  input rx_b; output tx_b;
  input rx_c; output tx_c;
}
channel Wire {
 parameters:
  @class("dir.ethernet.Link");
  double bitrate @unit(bps) = default(1Gbps);
  double delay @unit(s) = default(1000ps);
}
network Net {
 submodules:
  a: ethdynamic.Endpoint;
  b: ethdynamic.Endpoint;
  c: ethdynamic.Endpoint;
  s1: ethdynamic.Switch;
  s2: ethdynamic.Switch;
  s3: ethdynamic.Switch;
 connections:
  a.tx --> ethdynamic.Wire --> s1.rx_a;
  s1.tx_a --> ethdynamic.Wire --> a.rx;
  b.tx --> ethdynamic.Wire --> s2.rx_a;
  s2.tx_a --> ethdynamic.Wire --> b.rx;
  c.tx --> ethdynamic.Wire --> s3.rx_a;
  s3.tx_a --> ethdynamic.Wire --> c.rx;
  s1.tx_b --> ethdynamic.Wire --> s2.rx_b;
  s2.tx_b --> ethdynamic.Wire --> s1.rx_b;
  s1.tx_c --> ethdynamic.Wire --> s3.rx_b;
  s3.tx_b --> ethdynamic.Wire --> s1.rx_c;
  s2.tx_c --> ethdynamic.Wire --> s3.rx_c;
  s3.tx_c --> ethdynamic.Wire --> s2.rx_c;
}
