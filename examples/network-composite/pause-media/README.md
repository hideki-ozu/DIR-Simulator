# Configured media and MAC Control PAUSE

`model.ini` runs sparse configured 1000BASE-T1 timing on the A–S1 connection and ordinary ideal links on the remaining graph. S1 sends an actual 64-byte PAUSE frame to A at time zero. A's queued traffic starts after receipt and four physical-rate quanta. The model uses the existing network queues and single FES.

```sh
cargo run --release -- run --config examples/network-composite/pause-media/model.ini --output /tmp/pause-media-run
```

The repository CLI's `--help` describes the installed command syntax. Results include `dir.network.mac_control`, `dir.network.configured_media`, and the existing Ethernet transfer/reception records. An actual control transmit or receive has its own committed milestone; a planned arrival is not a receive.

`configured_media` reuses the existing physical-link contract:

```json
{"seed":"7","physical_links":[{"id":"access_a","a":"Net.a.tx","b":"Net.s1.tx_a","phy_mode":"1000base-t1","duplex":"full","a_phy":{"role":"master","tx_latency_ps":"123","rx_latency_ps":"234"},"b_phy":{"role":"slave","tx_latency_ps":"345","rx_latency_ps":"456"}}]}
```

Only listed outputs use configured media. A direction cannot also belong to a sampled PHY or shared domain. Legacy media profiles still require complete coverage.

`mac_control` declares station identity and administration independently of traffic classes:

```json
{"ports":[{"port":"Net.a.tx","station_mac":"02:00:00:00:00:01","rx_enabled":true,"tx_enabled":true,"policy":"ieee802.3.pause.v1"}],"controls":[{"at_ps":"1000000","port":"Net.a.tx","kind":"pause_request","quanta":1},{"at_ps":"2000000","port":"Net.a.tx","kind":"rx_enable","enabled":false}]}
```

A port's `tx_enabled` controls PAUSE generation, `rx_enabled` controls its local receive function, and the scheduled `transmit_enable` action controls new MAC requests. Supported actions are `pause_request`, `rx_enable`, `tx_enable`, and `transmit_enable`. Equal-time actions preserve configuration order. Generation uses reserved multicast DA `01:80:c2:00:00:01`, EtherType `0x8808`, opcode `1`, big-endian quanta, zero padding, and Ethernet FCS. It uses the existing control FIFO and selected physical owner. It never directly sets a peer timer.

The receive function consumes MAC Control before bridge processing. It accepts the reserved DA or the station's individual DA and discards unsupported opcodes and oversized frames (the discard alternative permitted by Clause31.5.1). Invalid FCS cannot load a timer. A new request replaces the old request. If a synchronous MAC request is in progress, its frame/retries finish first; the previous timer continues while the replacement is pending. Zero clears inhibition when processed. MAC FCS completion is distinct from sampled PCS-tail release.

Timers use physical time and `ceil(quanta * 512 * 10^12 / physical_rate)`, independently of node clocks. Project numerical Manchester media use their exact symbol-rate/2 MAC rate. Timer evidence publishes the rate numerator and denominator.

Only MAC Control bypasses PAUSE and data-class TAS/CBS. Ordinary protocol FIFO traffic uses encoded PCP or the output's default priority. Open-backlog CBS continues to accumulate during PAUSE/carrier/backoff; closed gates freeze. Existing capped credit accounting remains selected for configured ordinary MACs. A synchronous collision retry keeps ownership despite a subsequent gate closure. The explicit `dir.experimental.half_duplex_tsn.v1` policy is required for half-duplex shaping.

Standard PAUSE generation is rejected on affected half-duplex and Merge outputs. Disjoint outputs remain supported. Received synthetic inhibition on Merge requires `dir.experimental.merge_pause.v1`; this is an experimental receive policy and does not authorize IEEE-prohibited generation. PFC is not implemented; unknown capability/configuration fields reject.

RX-disable clears pending/active inhibition. TX-disable cancels queued PAUSE generation. Link epochs invalidate old generated controls and local pending/timer state; link-up starts unpaused. Existing submitted frame ownership remains intact.

Checkpoint codec19 includes typed configured domains, sparse endpoint bindings, retained work and retry state, actual receive evidence, PAUSE request/timer generations, and FIFO state. Restoration checks the exact committed prefix from frozen inputs in addition to structural state and FES consistency.

`sampled.ini` uses ordinary MACs on an actual full-duplex 1000BASE-T1 sampled link. It retains the link's real startup and sends a PAUSE while a 1518-byte frame is transmitting. The longer 6.05 ms horizon includes actual MAC completion, the coded tail and IFG. Execute this startup fixture with a release build.

Admission boundaries are explicit per existing physical API. Configured media can retain a provisional pre-SOF queue claim; its synchronous MAC request begins at first SOF and owns all collision retries until frame success/failure. The sampled adapter submits its request through the existing physical MAC `admit` API; that submitted request remains pending through physical deference. Its pre-SOF permissions are refreshed without creating another queue or retransmission engine.

Focused integration coverage is in `crates/dir-simulator/tests/network_pause_media.rs`. The legacy configured-media fixture catalog remains unchanged. Whole-network T06/N13 closure additionally requires the parent-owned Wire/Merge express adapters, configured-media gPTP timestamp adapter, fractional sampled-cut validation, and final integrated review.
