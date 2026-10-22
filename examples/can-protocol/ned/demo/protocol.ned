package demo;
simple Protocol { parameters: @class("dir.can.ProtocolV1"); }
network Demo { submodules: bus: demo.Protocol; }
