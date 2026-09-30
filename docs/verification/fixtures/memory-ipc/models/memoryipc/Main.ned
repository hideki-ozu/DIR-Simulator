package memoryipc;
network Main {
 submodules:
  ddr: Ddr;
  sram: Sram;
  shared: Shared;
  dma: Dma;
  mailbox: Mailbox;
}
