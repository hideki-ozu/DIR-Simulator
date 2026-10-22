# Selected IEEE 802.1CB-2017 vector recovery

Run from the repository root:

```sh
cargo run -p dir-simulator -- run --config examples/frer-rtag/normative/model.ini --output out/frer-normative
```

This opt-in sample selects `recovery_algorithm: "vector_8021cb_2017"` with an eight-bit history and a 1 ms reset timer (`recovery_timeout_ps: "1000000000"`). Omitted selection retains the existing project history behavior. Vector mode requires history capacity 2..32767 and a positive integral number of milliseconds.

The source sends one original frame. Two physical branches carry its R-TAG sequence 0 from the replication bridge to the elimination bridge. Branch B has a 2 ms propagation delay. Branch A is accepted, continued over the physical suffix, and delivered to the endpoint. The globally phased 1 ms recovery timer then expires without another arrival. Branch B is accepted after this reset and creates its own suffix continuation and endpoint delivery. The result has two accepted recoveries, two continuation rounds and two actual endpoint receptions for one original frame. Each round owns its received bytes, suffix transfers and delivery evidence.

The timer uses global simulation ticks at exact integral milliseconds. A timeout at the same timestamp as reception runs first. Accepted first, advance and late packets refresh the timer; duplicate and rogue inputs do not. A timeout resets recovery history without resetting the independent modulo-65536 sequence generator. Copies separated by a timeout can therefore be delivered again; this selected nonindividual algorithm does not promise indefinite duplicate suppression.

The selected subset comprises IEEE 802.1CB-2017 sequence generation, the six-octet R-TAG, and nonindividual vector recovery with TakeNoSequence=false. It excludes Match_Alg, individual recovery, autoconfiguration, latent-error detection, HSR, PRP, arbitrary stack positions and complete bridge/device conformance. Supported VLAN tags precede the R-TAG. Received FCS is checked before recovery; each continuation strips its own R-TAG and preserves payload/padding, adding padding when necessary to keep a minimum 64-byte untagged MAC frame.

Results expose `dir.redundancy.recovery`, `dir.redundancy.timeout`, the origin's `continuation_rounds`, and actual Ethernet transfer/reception records. The Viewer replays each round and delivery separately. The original `../model.ini` and `../distributed/model.ini` samples retain their project history selection and behavior.
