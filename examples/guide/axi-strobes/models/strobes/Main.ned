package strobes;
simple Manager {
 parameters: @class("dir.axi.Manager");
 gates: output aw; output w; input b; output ar; input r;
}
simple Interconnect {
 parameters: @class("dir.axi.Interconnect");
 gates:
  input aw_m0; input w_m0; output b_m0; input ar_m0; output r_m0;
  output aw; output w; input b; output ar; input r;
}
simple Ram {
 parameters: @class("dir.axi.Ram");
 gates: input aw; input w; output b; input ar; output r;
}
network Main {
 submodules: m0: strobes.Manager; bus: strobes.Interconnect; ram: strobes.Ram;
 connections:
  m0.aw --> bus.aw_m0;
  m0.w --> bus.w_m0;
  m0.ar --> bus.ar_m0;
  bus.b_m0 --> m0.b;
  bus.r_m0 --> m0.r;
  bus.aw --> ram.aw;
  bus.w --> ram.w;
  bus.ar --> ram.ar;
  ram.b --> bus.b;
  ram.r --> bus.r;
}
