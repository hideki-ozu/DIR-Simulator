package shared;
simple Buffer {
 parameters:
  @class("dir.ipc.SharedV1");
}
network Main {
 submodules:
  buf: shared.Buffer;
}
