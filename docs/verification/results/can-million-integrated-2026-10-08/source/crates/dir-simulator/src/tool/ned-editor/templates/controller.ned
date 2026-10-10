simple __NAME__ {
 parameters:
  @class("__CLASS__");
  int queueCapacity = default(64);
  double txProcessingDelay @unit(s) = default(0ps);
  double rxProcessingDelay @unit(s) = default(0ps);
  string rxFilter = default("*");
 gates:
  output tx;
  input rx;
}
