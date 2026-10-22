# Wire serializer independent oracle

文書ID：`verification-wire-serializers-fixtures`

`oracle.py` independently checks actual `results.json` emitted by the public
`examples/network-composite/wire-serializers` fixtures. It imports no product code.
It recomputes CRC-32 bit by bit, original Wire bytes/FCS, Clause99 cumulative
fragment mCRC, original payload slices, serialization/IPG timing, and sampled
GMII rational sample/ceiling timestamps. The sampled fixture origin is 12345 ps.
It also counts actual late PCS receive events after serializer release.

```sh
python3 docs/verification/fixtures/wire-serializers/oracle.py \
  /tmp/dir-wire-merge/results.json /tmp/dir-wire-sampled/results.json
```

Pass only actual complete fixture outputs with no injected transport faults.
The checks cover these bytes and timing relationships. They do not establish
full IEEE conformance, arbitrary PHY accuracy, abstract fragment wire encoding,
or closure of N13/T06 or all 81 capabilities. Runtime regression tests separately
exercise abstract scheduling, shaped ordinary control, typed MAC Control,
transaction rollback, frozen-input checkpoint replay, and negative mutations.
