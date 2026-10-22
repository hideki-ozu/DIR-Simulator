# R-TAG byte path

`model.ini` selects `ethernet.frer.rtag.v1`. It uses the existing registered network arena, the TSN queue scheduler and two configured replication routes. Each queued copy contains the six-byte R-TAG (`F1 C1`, zero reserved field, 16-bit sequence), followed by a recomputed Ethernet FCS. Every hop retransmits a rewritten frame and the terminal reads and validates the tag bytes before sequence recovery. The wire length includes the tag.

The sequence window is bounded below the 16-bit half range. It accepts wrap from 65535 to 0, rejects the ambiguous half-range distance, and resets after the configured idle timeout. This is an explicitly scoped simulator recovery policy. The example does not assert full IEEE 802.1CB conformance, physical PHY decoding, dynamic route selection or MAC Merge. The older `ethernet.redundancy.abstract.v1` profile retains its existing sequence and wire behavior.

The tag layout and reserved-field policy are corroborated by the [IEEE 802.1 working-group FRER R-TAG presentation](https://www.ieee802.org/1/files/public/docs2020/new-varga-FRER-RTag-0920-v01.pdf), page 3; that presentation is not the ratified standard text.
