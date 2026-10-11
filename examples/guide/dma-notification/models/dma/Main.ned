package dma;
simple Memory {
 parameters:
  @class("dir.memory.SramV1");
}
simple Engine {
 parameters:
  @class("dir.dma.EngineV1");
}
network Main {
 submodules:
  src: dma.Memory;
  dst: dma.Memory;
  dma: dma.Engine;
}
