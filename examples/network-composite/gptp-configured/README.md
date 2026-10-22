# Configured full-duplex gPTP

`model.ini` runs actual Pdelay, Announce, Sync and Follow_Up packets over a configured 1000BASE-T1 full-duplex link. Endpoint TX/RX latencies are 123/234ps and 345/567ps, with symmetric 100ns propagation and distinct raw clock phase/rate.

The selected configured plane is `MdiMacSfd`: TX reference is actual MAC SOF + endpoint TX latency + 64 bit times; RX reference adds propagation. Receive availability is actual EOF + TX latency + propagation + peer RX latency. Captures evaluate the immutable raw oscillator at the reference; actual serialized arrival and FCS validation release the packet protocol. Pdelay measures propagation in neighbor raw units.

`details.configured` binds each capture to its retained actual source and configuration. Planned EOF/arrival fields are labelled planned. EOF and TX capture may occur in either order; Follow_Up completion joins them exactly once. PAUSE and shaping affect admission before SOF.

This deterministic configured adapter does not provide sampled PHY, electrical training, autonegotiation or full IEEE conformance evidence. Half/shared domains and asymmetric propagation are outside the selected gPTP profile. The executable product gates are in `crates/dir-simulator/tests/network_gptp_configured.rs`.

`merge.ini` selects actual Clause99 EXPRESS source records; `abstract.ini` selects project abstract EXPRESS. `abstract-ideal.ini` uses the same abstract producer with declared `IdealMacSfd`, zero endpoint latency and the original 100ns propagation. All retain producer source identities after normal MAC release; configured captures use `MdiMacSfd`. `expected.json` contains independently derived first ordinary PdelayReq references, raw timestamps and availability cuts.
