package phy;
simple SampledMedium { parameters: @class("ethernet.SampledPhy"); }
network Demo { submodules: medium: phy.SampledMedium; }
