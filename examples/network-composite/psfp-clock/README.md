# Receiver-local PSFP gate

The same two 64-byte Ethernet frames arrive at b at576000ps and2576000ps. Its gate opens during the first500000ps of each1000000ps local cycle.

- identity.ini: receiver local time equals global, both packets are gated out.
- offset.ini: receiver offset+500000ps, both packets pass and are actually received.
- wide-offset.ini: receiver offset2^64ps; periodic phase is evaluated before narrowing, so both packets pass without a physical scheduler overflow.
- steps.ini: receiver starts with that offset, steps back500000ps at400000ps, then forward500000ps at1400000ps. The first packet is gated out and the second is received.

This isolates receiving-node gate time from unchanged sender/serialization times. No unrelated replication or preemption is enabled. It exercises explicitly configured clocks/corrections; it is not a gPTP wire-convergence claim. Token replenishment, when configured, uses physical elapsed time, and is independently checked against same-time clock steps in the runtime unit gate.
