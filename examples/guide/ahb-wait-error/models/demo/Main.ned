package demo;
simple Source { parameters: @class("dir.ahb.Manager"); gates: output request; input response; }
simple Target { parameters: @class("dir.ahb.Target"); gates: input request; output response; }
simple Bus { parameters: @class("dir.ahb.Bus"); gates: input request_m0; output response_m0; input request_m1; output response_m1; output request_t; input response_t; }
network Main { submodules: m0: demo.Source; m1: demo.Source; bus: demo.Bus; t: demo.Target;
 connections: m0.request --> bus.request_m0; bus.response_m0 --> m0.response; m1.request --> bus.request_m1; bus.response_m1 --> m1.response; bus.request_t --> t.request; t.response --> bus.response_t;
}
