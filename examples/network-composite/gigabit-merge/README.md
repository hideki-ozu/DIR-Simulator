# Continuous 1000BASE-T1 MAC Merge

This two-node example runs actual sampled Clause 97 startup before sending the
Clause 99 LLDP capability, Verify and Respond packets. At 6 ms a preemptable MAC
frame starts; two express arrivals interrupt it into three demand-driven fragments.

Run `dir-simulator run --config examples/network-composite/gigabit-merge/model.ini --output <directory>`
with the repository CLI. The unoptimized sampled startup takes several minutes.
The configuration uses the explicit `gigabit_raw_merge` xMII adapter and a 6.012 ms
simulation limit. It includes real PHY decoding, fragment CRC checks, exact xMII
EOF and twelve idle octets before MAC release. MDI coded completion is separate.

The output's `dir.mac_merge.packet` records identify actual xMII timings and raw
packet provenance. `dir.network.phy` records retain physical producer events and
actual decoded ingress. A planned ideal arrival is never used for this example.
Raw Merge plus gPTP and a sampled Qcc service guarantee remain separate admission
contracts until their timestamp/service bounds are established.
