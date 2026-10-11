package ddr;
simple Memory {
 parameters:
  @class("dir.memory.DdrV1");
}
network Main {
 submodules:
  mem: ddr.Memory;
}
