# RV32I Spike oracle fixture

`oracle.json` is captured from the pinned Berkeley Spike simulator. The Rust
integration test checks the pure RV32I evaluator against the actual committed
register, PC, load, and store effects emitted by Spike. The fixture includes 38
retiring RV32I instruction forms, one seeded 256-instruction arithmetic trace,
and a four-word sum program that branches, loads, and stores. Spike debug probes
also compare the sequential PC wrap and the cause, exception PC, and trap value
for CPU decode, memory, and fetch faults. It is a focused regression corpus; it
does not establish full ISA conformance.

Regenerate it with the same upstream source and built executable:

```sh
PATH=/tmp/dir-rv32i-oracle-tools/usr/bin:$PATH \
LD_LIBRARY_PATH=/tmp/dir-rv32i-oracle-tools/usr/lib/x86_64-linux-gnu:/tmp/dir-rv32i-oracle-build \
python3 crates/dir-simulator/tests/fixtures/rv32i_spike_oracle/generate.py \
  --spike /tmp/dir-rv32i-oracle-build/spike \
  --spike-repo /tmp/dir-rv32i-oracle-spike \
  --output crates/dir-simulator/tests/fixtures/rv32i_spike_oracle/oracle.json \
  --evidence-dir /tmp/dir-remaining-implementation-20261021-evidence/rv32i-oracle
```

The generator refuses any source or SoftFloat revision other than the pinned
commits recorded in the JSON. Its evidence directory contains the ELF programs,
raw Spike logs, and exact command arrays used to create the fixture.
