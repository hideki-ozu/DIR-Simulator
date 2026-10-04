package media;
simple Endpoint {
 parameters:
  @class("dir.ethernet.EndpointV2");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
 gates: output tx; input rx;
}
simple Switch {
 parameters:
  @class("dir.ethernet.SwitchV2");
  int queueCapacity = default(64);
 gates:
  input rx_a; output tx_a;
  input rx_b; output tx_b;
  input rx_c; output tx_c;
}
channel Link {
 parameters:
  @class("dir.ethernet.LinkV2");
  double bitrate @unit(bps) = default(100Mbps);
  double delay @unit(s) = default(100ns);
}
