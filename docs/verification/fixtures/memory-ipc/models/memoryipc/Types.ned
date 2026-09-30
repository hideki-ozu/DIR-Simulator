package memoryipc;
simple Ddr {
 parameters:
  @class("dir.memory.DdrV1");
}
simple Sram {
 parameters:
  @class("dir.memory.SramV1");
}
simple Shared {
 parameters:
  @class("dir.ipc.SharedV1");
}
simple Dma {
 parameters:
  @class("dir.dma.EngineV1");
}
simple Mailbox {
 parameters:
  @class("dir.ipc.MailboxV1");
}
