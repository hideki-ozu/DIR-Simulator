package memoryipc;
network Main {
 submodules:
  ddr: memoryipc.Ddr;
  sram: memoryipc.Sram;
  shared: memoryipc.Shared;
  dma: memoryipc.Dma;
  mailbox: memoryipc.Mailbox;
}
