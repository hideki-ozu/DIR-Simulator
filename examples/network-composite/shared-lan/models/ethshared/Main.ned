package ethshared;
simple Endpoint {
 parameters: @class("dir.ethernet.Endpoint"); int queueCapacity = default(64); double txProcessingDelay @unit(s) = default(0ps); double rxProcessingDelay @unit(s) = default(0ps);
 gates: output tx; input rx;
}
simple Switch {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_a; output tx_a; input rx_b; output tx_b;
}
simple Hub {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_a; output tx_a; input rx_b; output tx_b; input rx_c; output tx_c;
}
channel Wire { parameters: @class("dir.ethernet.Link"); double bitrate @unit(bps) = default(1Gbps); double delay @unit(s) = default(1000ps); }
network Net {
 submodules:
  a: ethshared.Endpoint; b: ethshared.Endpoint; c: ethshared.Endpoint;
  s1: ethshared.Switch; s2: ethshared.Switch; s3: ethshared.Switch; hub: ethshared.Hub;
 connections:
  a.tx --> ethshared.Wire --> s1.rx_a; s1.tx_a --> ethshared.Wire --> a.rx;
  b.tx --> ethshared.Wire --> s2.rx_a; s2.tx_a --> ethshared.Wire --> b.rx;
  c.tx --> ethshared.Wire --> s3.rx_a; s3.tx_a --> ethshared.Wire --> c.rx;
  s1.tx_b --> ethshared.Wire --> hub.rx_a; hub.tx_a --> ethshared.Wire --> s1.rx_b;
  s2.tx_b --> ethshared.Wire --> hub.rx_b; hub.tx_b --> ethshared.Wire --> s2.rx_b;
  s3.tx_b --> ethshared.Wire --> hub.rx_c; hub.tx_c --> ethshared.Wire --> s3.rx_b;
}
simple Switch3 {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_a; output tx_a; input rx_b; output tx_b; input rx_c; output tx_c;
}
simple Hub4 {
 parameters: @class("dir.ethernet.Switch"); int queueCapacity = default(64);
 gates: input rx_a; output tx_a; input rx_b; output tx_b; input rx_c; output tx_c; input rx_d; output tx_d;
}
network Backup {
 submodules:
  a: ethshared.Endpoint; b: ethshared.Endpoint; c: ethshared.Endpoint;
  s1: ethshared.Switch3; s2: ethshared.Switch; s3: ethshared.Switch; hub: ethshared.Hub4;
 connections:
  a.tx --> ethshared.Wire --> s1.rx_a; s1.tx_a --> ethshared.Wire --> a.rx;
  b.tx --> ethshared.Wire --> s2.rx_a; s2.tx_a --> ethshared.Wire --> b.rx;
  c.tx --> ethshared.Wire --> s3.rx_a; s3.tx_a --> ethshared.Wire --> c.rx;
  s1.tx_b --> ethshared.Wire --> hub.rx_a; hub.tx_a --> ethshared.Wire --> s1.rx_b;
  s2.tx_b --> ethshared.Wire --> hub.rx_b; hub.tx_b --> ethshared.Wire --> s2.rx_b;
  s3.tx_b --> ethshared.Wire --> hub.rx_c; hub.tx_c --> ethshared.Wire --> s3.rx_b;
  s1.tx_c --> ethshared.Wire --> hub.rx_d; hub.tx_d --> ethshared.Wire --> s1.rx_c;
}
