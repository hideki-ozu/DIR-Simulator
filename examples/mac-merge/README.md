# IEEE 802.3-2022 Clause 99 adapter example

`model.ini` selects `ethernet.network.composite.v1` with the explicitly named
`mac_merge.standard = ieee8023_2022_clause99`. The two real physical directions
send LLDP capability packets, then real Verify/Respond packets through their
single serializer before preemption becomes active. Configuration alone cannot
activate preemption.

The 184-octet pMAC starts at 4,000,000 ps. Express demand arrives after SOF at
4,300,000 ps and again at 5,500,000 ps. With addFragSize=0, each actual fragment
contains 60 original data octets and a four-octet cumulative mCRC or original
final FCS. The expected physical intervals at 1 Gb/s are:

| mPacket | SOF ps | EOF ps | release after IPG ps |
| --- | ---: | ---: | ---: |
| pMAC first | 4,000,000 | 4,576,000 | 4,672,000 |
| express 0 | 4,672,000 | 5,248,000 | 5,344,000 |
| pMAC continuation 0 | 5,344,000 | 5,920,000 | 6,016,000 |
| express 1 | 6,016,000 | 6,592,000 | 6,688,000 |
| pMAC final continuation 1 | 6,688,000 | 7,264,000 | 7,360,000 |

Only actual final RX assembly and full-frame FCS validation permit normal
receive processing. Every mPacket, including LLDP and verification, is visible
under `dir.mac_merge.packet`. Assembly and verification records distinguish
incomplete state, failures, peer evidence, and active state.

The initial adapter supports ideal full-duplex 100 Mb/s and 1 Gb/s links, all
four addFragSize values, enabled verification, queue-only TSN, and real R-TAG
composition. TAS/CBS need separate actual-segment integration. Sampled PHY,
shared/collision media, wire-control, CAN conversion, Qcc, provider, multicast,
and local-clock ownership are currently rejected by this adapter.

`dir_lldp_capability_v1` is an explicit bounded real-packet capability adapter:
it emits and checks mandatory chassis/port/TTL TLVs, the Clause 79.3.7 extension,
and an End TLV, and expires actually received evidence using the packet's TTL.
Its complete IEEE 802.1AB source verification remains provisional. This example
does not claim full LLDP, PHY/xMII, device-delay, or production conformance.
