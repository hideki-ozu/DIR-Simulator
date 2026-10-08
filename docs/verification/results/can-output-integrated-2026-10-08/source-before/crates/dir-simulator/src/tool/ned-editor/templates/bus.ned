simple __NAME__ {
 parameters:
  @class("__CLASS__");
  double bitrate @unit(bps) = default(500kbps);
  string profile = default("__PROFILE__");
 gates:
  input rx_a;
  output tx_a;
  input rx_b;
  output tx_b;
}
