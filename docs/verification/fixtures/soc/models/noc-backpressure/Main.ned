package demo;
simple Endpoint { parameters: @class("dir.noc.Endpoint"); gates: output packet; input delivery; }
simple Router00 { parameters: @class("dir.noc.Router"); gates: input local_in; output local_out; input in_east; output out_east; }
simple Router10 { parameters: @class("dir.noc.Router"); gates: input local_in; output local_out; input in_west; output out_west; }
network Main { submodules: r00: demo.Router00; e00: demo.Endpoint; r10: demo.Router10; e10: demo.Endpoint; connections: r00.out_east --> r10.in_west; e00.packet --> r00.local_in; r00.local_out --> e00.delivery; r10.out_west --> r00.in_east; e10.packet --> r10.local_in; r10.local_out --> e10.delivery; }
