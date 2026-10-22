package noc;
simple Flit {
    parameters:
        @class("dir.noc.Flit");
}
network Demo {
    submodules:
        noc: noc.Flit;
}
