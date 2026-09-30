package demo;
simple Manager {
 parameters: @class("dir.axi.Manager");
 gates: output aw; output w; input b; output ar; input r;
}
simple Interconnect {
 parameters: @class("dir.axi.Interconnect");
 gates:
  input aw_m0; input w_m0; output b_m0; input ar_m0; output r_m0;
  input aw_m1; input w_m1; output b_m1; input ar_m1; output r_m1;
  output aw; output w; input b; output ar; input r;
}
simple Ram {
 parameters: @class("dir.axi.Ram");
 gates: input aw; input w; output b; input ar; output r;
}
network Main {
 submodules: m0: demo.Manager; m1: demo.Manager; bus: demo.Interconnect; ram: demo.Ram;
 connections:
  m0.aw --> bus.aw_m0;
  m0.w --> bus.w_m0;
  m0.ar --> bus.ar_m0;
  bus.b_m0 --> m0.b;
  bus.r_m0 --> m0.r;
  m1.aw --> bus.aw_m1;
  m1.w --> bus.w_m1;
  m1.ar --> bus.ar_m1;
  bus.b_m1 --> m1.b;
  bus.r_m1 --> m1.r;
  bus.aw --> ram.aw;
  bus.w --> ram.w;
  bus.ar --> ram.ar;
  ram.b --> bus.b;
  ram.r --> bus.r;
}
