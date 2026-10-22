# Sampled raw Merge + Qcc

`model.ini` runs the real 750MBd /16-sample-per-UI 1000BASE-T1 startup and raw Clause99 transport. Both ends exchange actual LLDP, Verify and Respond packets. Qcc prepares at6.02ms after both enabled advertisements, activates at6.1ms, and the managed source is generated at6.1ms. Its first SOF and decoded delivery are checked against the installed bound.

Untagged LLDP uses each port's ordinary default priority. Net.a.tx uses default7 so its advertisements can bootstrap while Qcc keeps managed class0 closed before installation. Net.b.tx is unmanaged and retains default0 so received untagged slow traffic stays in class0. The slow source keeps its explicit priority0 and untagged frame.

The guarantee requires the source-certified, fixed-clock, unity-single-tap, noiseless profile. Static transport reservation is4080ns plus the declared cadence-displacement upper bound and1ps FES rounding. Every LLDP/Verify/Respond admission is charged its actual wire/IPG occupancy plus8ns. The plan records actual physical epochs, configuration hashes and bilateral capability proof. Readiness loss withdraws the guarantee; reconnection requires fresh proof and an explicit new prepare.

Run the expensive production gate with a copied release `network_qcc` test executable and `--exact qcc_raw_certified_startup_installs_and_delivers_actual_sampled_stream --ignored`. This is a restricted executable model contract, not full IEEE conformance certification. N13 remains open.
