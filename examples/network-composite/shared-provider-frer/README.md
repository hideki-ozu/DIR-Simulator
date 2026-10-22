# Shared carrier, provider service and FRER

This composite example sends one unicast frame from `Net.s` to `Net.d`. `Net.r`
encapsulates customer VID 10 into service VID 100 and replicates it onto the
explicit `a` and `b` paths. `Net.e` recovers the stream, removes the service tag,
and sends one continuation to the destination.

The `a` branch uses a shared carrier through `Net.h`. One transmission from
`Net.r.tx_a` produces actual receptions at `Net.a.rx_r` and `Net.b.rx_x`.
Only `Net.a` is the declared next hop for this copy; the extra reception is
filtered with `replication_unintended_receiver`. `Net.h` owns the physical
junction and does not act as a forwarding bridge. The independent `b` path
still reaches the recovery point, where its duplicate is suppressed.

Provider network ports admit only S-VID 100. Customer ports and endpoints admit
only C-VID 10. The branch wire contains the S-tag, C-tag and R-TAG in that order;
provider forwarding preserves the actual customer payload and FRER sequence.

```sh
cargo run -p dir-simulator -- run \
  --config examples/network-composite/shared-provider-frer/model.ini \
  --output /tmp/shared-provider-frer-results
```

For the configured frame, the prefix and suffix MAC frames are 68 bytes
(FCS `f53e5771`); branch MAC frames are 78 bytes (FCS `9cb77d39`). Exactly six
transmissions occur: one prefix, two copies with two hops each, and one suffix.
The shared transmission starts at 611000 ps, ends at 1299000 ps, and produces
two independent receiving-MAC indications. The suffix starts at 1993000 ps.
This example exercises an ideal shared MAC service; it does not claim sampled
PHY, MAC Merge, or Qcc admission coverage.
