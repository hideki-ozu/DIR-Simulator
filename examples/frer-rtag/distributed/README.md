# Distributed replication and recovery

Explicit `bridge_pair_v1` placement: source s → prefix → replicator r → independently queued a/b paths → recovery bridge e → suffix → destination d. All links are 1Gbps with1000ps propagation, switches process for2000ps and endpoint d for3000ps. The source has one46-byte payload and a C-VID10 tag:68-byte ordinary MAC and74-byte R-TAG MAC on branch hops.

The project routing adapter keeps physical link, VLAN admission, PSFP, queue and TSN ownership, and selects declared physical circuits rather than dynamic FDB/tree routing. It does not claim full IEEE802.1CB conformance. Placement implementation and production acceptance are still in progress.

Expected baseline from independent timing arithmetic: prefix arrival609000ps, r branch SOF611000ps, first-hop arrival1268000ps, second-hop SOF1270000ps, eliminator arrival1927000ps, exactly one suffix SOF1929000ps, destination arrival2538000ps, final processing/delivery2541000ps. Exactly six physical transfers: one prefix, four branch hops, one suffix. Recovery alone must not announce terminal delivery. These are acceptance expectations until the actual owner test passes.
