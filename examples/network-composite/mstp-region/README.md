# MSTP per-VID forwarding over a cycle

All three bridges share the same MCID. The CIST root is s1, MSTI1 root s2, MSTI2 root s3. C-VID10 maps to MSTI1; C-VID20 maps to MSTI2. This topology is the existing physical triangle, with one endpoint on each bridge.

Packets from a to c begin at32/33 seconds, beyond two default15-second forwarding delays. This fixture does not claim a particular rapid-convergence time. Its acceptance distinguishes actual VLAN-specific forwarding from a CIST-only prefilter:

- VID10 reaches c via s1→s2→s3, including the s3 port blocked by CIST.
- VID20 reaches c via s1→s3. Each frame is delivered once with original payload/tag and a valid FCS.
- Learning on an MSTI-forwarding port works even when its CIST is Alternate.
- Checkpoint during protocol convergence and before/within data transfer reproduces the exact full run. Altered projection cannot override the saved protocol state.

The fixture is under active protocol validation. Do not infer standards closure from configuration acceptance, BPDU codec tests or displayed roles.
