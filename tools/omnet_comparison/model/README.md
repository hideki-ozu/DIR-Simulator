# OMNeT++ comparison adapter

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | Gateway試験アダプター、有限RX・TX待機・停止境界の実行方法を追加。文書版を1.0.0、対象タグをv1.0.0に統一 |
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
events are canceled and deleted by their owning module on teardown. Gateway
processing timers and copies waiting outside the native TX queue are also
released by their owning recorder on teardown.

## Gateway inputs and queues

`routingFile` defaults to an empty string for the original single-bus network.
A caller may generate a network with multiple native `CanBus` instances, each
with its own bandwidth, and set `routingFile` to a TSV with this exact header:

```text
kind	gateway	ingress	route_id	egress	format	id_min	id_max	processing_ps	rx_capacity	hop_limit
```

A `port` row declares every Gateway port, including ports with no routes. Its
route fields (`route_id` through `id_max`) are empty; its integer configuration
fields are populated. A `route` row describes one egress branch. Configuration
must agree across all rows belonging to a Gateway. Ingress and egress values
are exact `nodeLabel` strings. Routes match format and an inclusive CAN ID
interval; branches are sorted independently of input row order. All ports
register the raw source IDs and route boundaries with native FiCo input ports;
copies retain the raw ID, so wide intervals need no expanded ID table.

After an accepted `received` event, each Gateway ingress admits a parent into
its finite RX queue, or rejects the newest arrival with `rx_queue_full`.
Unmatched admitted parents record `no_route` and immediately release RX. Each
matching egress gets one pending branch and one child at Gateway processing
completion, unless the next hop exceeds `hop_limit`. Child identity is
`gw:<parent>/<gateway>/<route_id>/<egress>`, and RX identity is
`rx:<parent>/<gateway>/<ingress>`. Child source and TX channel delay come from
the egress port; origin and parent lineage survive native duplication.

RX remains occupied through Gateway processing, egress TX processing and any
wait for a positive-capacity native TX queue. Ready copies wait outside native
FiCo in FIFO order per egress, across ingress ports. The native SOF callback
schedules a separate same-time event to admit copies after the bus finishes
its grant callbacks. Capacity zero drops the submitted child with
`queue_full`; a hop-limit terminal branch has no child. RX releases exactly
once, after all egress branches are admitted or terminal. Ordinary raw-source
TX arrivals still drop when their native pending queue is full. Native
arbitration, frame length, inter-frame gap and initial one-bit wait remain
unchanged.

## Output

CSV header:

```text
event,time_ps,node,request_id,source,format,can_id,payload_hex,native_bits,queue_waiting,sequence,origin_request_id,parent_request_id,hops,gateway,ingress,egress,route_id,buffer_id,reason,rx_used,child_request_id
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

Gateway events are `rx_admitted`, `rx_dropped`, `rx_released`,
`forward_pending`, `forward_submitted`, `forward_dropped` (hop limit),
`route_filtered` and `waiting_tx`. Forwarding and RX rows retain the received
parent's request ID and immediate source; `child_request_id` is populated at
submission. Common child events and `waiting_tx` use the child request ID.
`rx_used` is the ingress occupancy after the recorded Gateway event.
`sequence` starts at zero and orders same-time observations. Native request
lineage starts with its own origin ID, an empty parent and zero hops.
