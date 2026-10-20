package media;
network Main {
 submodules: a: media.Endpoint; b: media.Endpoint;
 connections:
  a.tx --> media.Link --> b.rx;
  b.tx --> media.Link --> a.rx;
}
