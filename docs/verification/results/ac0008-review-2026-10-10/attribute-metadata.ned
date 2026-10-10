package attributecheck;
simple Endpoint {
 parameters:
  @class("dir.ethernet.Endpoint");
  @display("日本語 \"引用\" \\");
  @description("配送期限");
  int queueCapacity = default(64) @description("parameter 所有者");
 gates: output tx; input rx;
}
network Main {
 submodules: a: attributecheck.Endpoint; b: attributecheck.Endpoint;
}
