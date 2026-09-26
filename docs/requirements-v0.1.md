# DIR Simulator — v0.1 Requirements

## 1. Purpose

Rustで独立した離散イベント型シミュレーションエンジンを実装する。

対象は外部ネットワークだけでなく、SoC内部の通信・共有資源も含む。

初期対象:

- CAN / CAN-FD
- Ethernet
- Gateway
- Switch
- SoC Bus
- AXI / AHB相当の内部通信
- NoC
- DDR / SRAM
- Shared Memory IPC
- DMA
- Mailbox IPC

OMNeT++の完全互換は目標にしない。

## 2. Core Design Policy

```text
NED
= 構造・型・接続・デフォルトParameter

INI
= Simulation ScenarioごとのParameter Override

Rust Module
= 実際の振る舞い

Module Registry
= NED Type と Rust実装の対応

Result Layer
= Metricの記録と出力
```

## 3. NED Compatibility Scope

### Supported in v0.1

- NED lexical syntax
- simple module
- compound module
- submodules
- named input/output gate
- connections
- parameters
- package declaration
- fully qualified type name

### Deferred

- module inheritance
- dynamic module creation
- conditional submodules
- module vectors
- gate vectors
- import
- wildcard import
- for-loop connection generation
- conditional connection
- automatic gate expansion

### Explicit Non-Goals

- OMNeT++ runtime API compatibility
- C++ `cSimpleModule` compatibility
- `cMessage` compatibility
- `.msg` compiler compatibility
- INET binary compatibility
- INET C++ API compatibility
- OMNeT++ implementation source reuse

## 4. NetworkNode Model

すべてのSimulation要素は共通の `NetworkNode` 概念を持つ。

対象例:

- ECU
- Gateway
- Switch
- CAN Bus
- Ethernet Link
- CPU Core
- DMA
- AXI Interconnect
- Memory Controller
- DDR
- Shared Memory
- IPC Endpoint

固定的な「Endpoint / Transport」分類は行わず、各Nodeが持つCapabilityとPortによって役割を表現する。

## 5. Capability Model

候補:

```text
CanEndpoint
EthernetEndpoint
Buffered
Forwarder
ProtocolConverter
Arbitrator
DelayProvider
SharedResource
MemoryInitiator
MemoryTarget
IpcEndpoint
ProcessingNode
```

CapabilityはRust側のtraitとして実装する。

## 6. Port / Gate Model

v0.1で対応:

- input
- output
- named gate
- connection validation

v0.1では非対応:

- inout
- gate vector
- dynamic gate creation

外部接続の互換性はNode全体ではなくPortの型で判定する。

候補:

```text
CanTx
CanRx
EthernetTx
EthernetRx
MemoryRequest
MemoryResponse
IpcTx
IpcRx
```

## 7. Connection Model

Connectionはトポロジーを表現する。

```text
source port
    ↓
target port
```

Validation:

- source node exists
- source port exists
- target node exists
- target port exists
- direction compatibility
- protocol / port type compatibility
- illegal duplicate detection

伝送遅延、帯域、Arbitration等はConnectionそのものではなく、Node / Resource側に持たせる。

## 8. Parameter Model

NED ParameterはModule Typeの共通Defaultとして扱う。

INIはInstance生成時に上書きする。

v0.1の優先順位:

```text
NED Type Default
        ↓
INI Instance Override
```

NED Instance Overrideは後回し。

## 9. INI Role

INIはSimulation Scenarioを定義する。

```ini
[simulation]
duration = 10s

[node.gateway]
processingDelay = 20us
bufferSize = 64

[node.canBus]
bitrate = 500kbps

[node.ddr]
latency = 80ns
bandwidth = 20GBps
```

```text
NED = System Structure
INI = Experiment / Scenario
```

## 10. Parameter Types

候補:

```text
Integer
Float
Bool
String
Time
DataRate
DataSize
Frequency
Count
Probability
```

型不整合はロード時にエラーとする。

## 11. Delay Model

Delayは発生源ごとに持つ。

- Gateway: processing / conversion / routing / buffering delay
- Ethernet Link: serialization / propagation / bandwidth
- CAN Bus: arbitration / frame transmission / contention
- SoC Bus / NoC: arbitration / queueing / transfer
- Memory: controller queue / access latency / transfer

概念式:

```text
total_delay
=
fixed_latency
+
queueing_delay
+
arbitration_delay
+
size / bandwidth
```

## 12. Buffer Model

属性候補:

```text
capacity
occupancy
overflow_policy
priority_policy
```

満杯時のPolicy:

- drop
- block
- backpressure
- retry

## 13. Shared Resource / Arbitration

対象例:

- CAN Bus
- AXI Interconnect
- NoC
- Memory Controller

Arbitration候補:

- fixed priority
- round robin
- weighted round robin
- QoS
- credit based

## 14. Message / Transaction Model

`.msg` compiler互換は実装しない。

Rust-native型の候補:

- CanFrame
- EthernetFrame
- MemoryRequest
- MemoryResponse
- IpcMessage

共通Envelope:

```text
MessageEnvelope
├── id
├── created_at
└── payload
```

## 15. Module Registry

```text
automotive.gateway.CentralGateway
        ↓
Module Registry
        ↓
Rust Factory
        ↓
CentralGateway Instance
```

完全修飾名をRegistry Keyとして利用可能にする。

## 16. Package Policy

v0.1:

- package declaration
- fully qualified type name

importは後回し。

## 17. Result Model

Time-Series Metric:

- queue_length
- latency
- utilization
- throughput
- arbitration_delay

Scalar Metric:

- average_latency
- max_queue_length
- drop_count
- average_utilization

## 18. Result Export

v0.1:

- CSV
- JSON

後回し:

- Parquet
- OMNeT++ `.vec`
- OMNeT++ `.sca`

## 19. INET Policy

非目標:

- binary compatibility
- C++ API compatibility
- model implementation reuse

将来候補:

- INET NED import
- INET Model Mapping

## 20. v0.1 Acceptance Criteria

1. NEDからsimple / compound moduleを読み込める
2. submoduleを展開できる
3. named input/output portを生成できる
4. connectionを解決・検証できる
5. NED Default Parameterを保持できる
6. INIからInstance Parameterを上書きできる
7. Module Registry経由でRust実装を生成できる
8. Event Schedulerが動作する
9. Message / TransactionをNode間で転送できる
10. DelayをSimulation Timeへ反映できる
11. BufferとShared Resourceを扱える
12. Arbitration Policyを適用できる
13. Scalar / Time-Series Metricを記録できる
14. CSV / JSONへ結果を出力できる

## 21. Development Principles

1. NEDは構造記述に集中させる
2. INIはScenario差分に使う
3. 振る舞いはRust側に持たせる
4. すべての要素をNetworkNodeとして扱えるようにする
5. Capabilityはtraitとして追加する
6. 接続互換性はPortで検証する
7. Delay / Buffer / Arbitrationの責務を分離する
8. CAN / Ethernet / SoC / IPCを同じEvent Kernelで扱う
9. OMNeT++ runtime API互換は追わない
10. 公開仕様から独立実装する
11. Internal IRと入出力形式を分離する
12. 初期仕様を小さく保つ
