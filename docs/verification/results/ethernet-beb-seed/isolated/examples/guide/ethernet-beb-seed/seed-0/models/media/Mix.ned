package media;
network Mix {
 submodules: a: media.Endpoint; b: media.Endpoint; c: media.Endpoint; sw: media.Switch;
 connections:
  a.tx --> media.Link --> sw.rx_a;
  sw.tx_a --> media.Link --> a.rx;
  b.tx --> media.Link --> sw.rx_b;
  sw.tx_b --> media.Link --> b.rx;
  c.tx --> media.Link --> sw.rx_c;
  sw.tx_c --> media.Link --> c.rx;
}
