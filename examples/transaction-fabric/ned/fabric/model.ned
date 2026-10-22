package fabric;
simple Fabric { parameters: @class("fabric.Concurrent"); }
network Demo { submodules: fabric: fabric.Fabric; }
