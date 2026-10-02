# OMNeT++ comparison adapter

文書バージョン：`0.1.1`
対象GitHubバージョン：`v0.1`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.1` | `2026-10-03` | Classical CANのCLI・ライブラリ、結果ビューア、実行例、OMNeT++比較の利用方法を集約し、v0.1公開版を確定 |

This is a test-only adapter authored for DIR-Simulator. It links the installed
FiCo4OMNeT library; it does not copy that library's implementation into DIR or
modify DIR's simulator, OMNeT++, or FiCo. Bus, arbitration, frame validation,
frame length and transmit/receive timing use FiCo's existing APIs unchanged.
The adapter is therefore **adapted FiCo**, not an unmodified upstream benchmark.

Added behavior is a raw-request source, fixed TX processing delay, a finite
pending queue, request identity instrumentation, post-native-completion channel
delays, an input filter and fixed RX processing delay. `MOB=false` preserves
repeated same-ID requests. Queue capacity counts waiting frames and excludes
`currentFrame`; before the first native grant, all contenders are still waiting.
An admission that finds capacity full is dropped before registering with FiCo.

FiCo's native model at `bitStuffingPercentage=0` uses its own fixed Classical CAN
frame length. It does not compute content-dependent CRC/stuffing. Its
`native_complete` and `native_rx_complete` include the inter-frame gap: native EOF
and bus release cannot be observed separately. The existing bus also waits one
bit time from the first idle registration to its first grant. The adapter keeps
both behaviors, without compensating timestamps to match DIR. `sof` is the
native sending-permission callback, at which the frame is sent to the native
port at the same timestamp. These limitations are part of the comparison.

## Inputs and invocation

Each node's UTF-8 TSV uses this exact header:

```text
generation_ps	request_id	format	can_id	payload_hex
```

Times and identifiers are decimal nonnegative integers. `format` is `standard`
or `extended`. `payload_hex` is an even-length string of up to sixteen hex
digits, or `-` for an empty payload. Request IDs must be unique within a node.
No DIR output is consumed. Explicit and periodic traffic must be expanded from
the original configuration into raw generations by the caller.

Build in an ignored directory, with an existing OMNeT++/FiCo installation:

```bash
OMNET_WORKSPACE=/home/hideki/Omnet++ BUILD_DIR="$PWD/tmp/omnet-adapter-build" \
    tools/omnet_comparison/model/build.sh
source /home/hideki/Omnet++/scripts/env.sh
opp_run -u Cmdenv \
    -n "$PWD/tools/omnet_comparison/model:/home/hideki/Omnet++/upstream/FiCo4OMNeT/src" \
    -l /home/hideki/Omnet++/upstream/FiCo4OMNeT/src/FiCo4OMNeT \
    -l "$PWD/tmp/omnet-adapter-build/DirOmnetAdapter" -f /path/to/omnet.ini
```

The network is `dir.omnetcomparison.AdapterNetwork`. Required INI settings are
`simtime-resolution=ps`, `*.horizonPs`, `*.outputFile`, and, for each
`*.node[i]`, `nodeLabel` and `sourceFile`. Network defaults are `nodeCount=3` and
`bitrate=500000` bits/s. Node parameters `queueCapacity` (default 16),
`txProcessingPs`, `rxProcessingPs`, `txChannelPs` and `rxChannelPs` (default zero)
are nonnegative integers. `rxFilter` accepts `*`, `none`, or comma/space-separated
IDs, optionally prefixed by `standard:`/`extended:`. Filter IDs may be decimal or
hex with `0x`. A plain ID accepts either format. Filtering occurs after native
receive completion plus source TX channel and destination RX channel delay;
accepted frames then incur RX processing delay. TX channel delay never delays
the native SOF. Every node registers all input IDs with its native input port so
filter decisions and observations remain visible. Native FiCo suppresses sender
self-reception and rejects equal-ID collisions between distinct transmitters.

All adapter transitions use the OMNeT++ future event set, including zero-delay
processing. A highest-priority stop event makes the horizon exclusive; events at
`H` are not executed. `H=0` produces a header-only CSV. Pending source and sink
events are canceled and deleted by their owning module on teardown.

## Output

CSV header:

```text
event,time_ps,node,request_id,source,format,can_id,payload_hex,native_bits,queue_waiting
```

Events are `generated`, `ready`, `enqueued`, `dropped`, `sof`, `native_complete`,
`native_rx_complete`, `observed`, `filtered` and `received`. All times are integer
picoseconds measured in the current OMNeT++ run. Request ID and original source
survive native frame duplication. Format and ID are read from frame metadata;
`payload_hex` is computed from the actual FiCo payload byte array at each event,
including after native duplication. Empty payload is an empty CSV string.
`queue_waiting` is populated for `ready` (before delivery to the
buffer), `enqueued` (after admission), `dropped`, `sof` (after grant), and
`native_complete` (before removal of the active frame). Each value excludes the
active frame. Use `enqueued` and `sof` for queue-state integration; `ready` events
at the same time may precede delivery to the native buffer.
