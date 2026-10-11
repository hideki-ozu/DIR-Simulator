package sram;
simple Memory {
 parameters:
  @class("dir.memory.SramV1");
}
network Main {
 submodules:
  sram: sram.Memory;
}
