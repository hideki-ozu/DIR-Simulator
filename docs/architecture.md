# DIR Simulator — Rust Architecture and API Design

## 1. Architecture Overview

```text
                NED
                 │
                 ▼
          Lexer / Parser
                 │
                 ▼
               AST
                 │
                 ▼
            Model IR
                 ▲
                 │
               INI
       Parameter Override
                 │
                 ▼
        Resolved Model
                 │
                 ▼
        Module Registry
                 │
                 ▼
        Rust NetworkNodes
                 │
                 ▼
       Simulation Runtime
      ┌──────────┼──────────┐
      ▼          ▼          ▼
    Queue     Resource   Arbitration
      │          │          │
      └──────────┴──────────┘
                 │
                 ▼
             Metrics
                 │
        ┌────────┼────────┐
        ▼        ▼        ▼
       CSV      JSON    Parquet
```

## 2. Suggested Workspace Structure

```text
workspace/
├── sim-core/
├── ned-parser/
├── sim-model/
├── sim-config/
├── sim-runtime/
├── sim-result/
├── sim-can/
├── sim-ethernet/
├── sim-soc/
├── sim-memory/
├── sim-ipc/
└── sim-cli/
```

## 3. Core IDs

```rust
struct NodeId(u64);
struct PortId(u64);
struct ConnectionId(u64);
struct EventId(u64);
struct MessageId(u64);
struct MetricId(u64);
```

内部参照は文字列だけに依存せずID化する。

## 4. Simulation Time

```rust
struct SimTime(u64);
```

浮動小数の直接利用を避ける。Time ParameterのParseとRuntime表現を分離する。

## 5. Event Model

```rust
struct Event {
    id: EventId,
    time: SimTime,
    target: NodeId,
    kind: EventKind,
}
```

```rust
enum EventKind {
    Initialize,
    Message {
        port: PortId,
        message: MessageEnvelope,
    },
    Timer {
        timer_id: u64,
    },
}
```

Future Event SetはPriority Queueで管理し、同時刻Eventの決定性のため `(time, sequence_number)` をKeyとする。

## 6. Scheduler

```rust
trait Scheduler {
    fn schedule(&mut self, event: Event);
    fn pop_next(&mut self) -> Option<Event>;
    fn now(&self) -> SimTime;
}
```

## 7. NetworkNodeCore

```rust
struct NetworkNodeCore {
    id: NodeId,
    name: String,
    ports: Vec<Port>,
    parameters: ParameterSet,
}
```

Rustの継承は使わずcompositionを採用する。

## 8. NetworkNode Trait

```rust
trait NetworkNode {
    fn core(&self) -> &NetworkNodeCore;
    fn core_mut(&mut self) -> &mut NetworkNodeCore;

    fn initialize(&mut self, ctx: &mut SimContext);

    fn on_message(
        &mut self,
        port: PortId,
        message: MessageEnvelope,
        ctx: &mut SimContext,
    );
}
```

最小APIは小さく保ち、必要な機能は追加traitへ分離する。

## 9. Capability Traits

候補:

```rust
trait Buffered {}
trait Forwarder {}
trait ProtocolConverter {}
trait Arbitrator {}
trait DelayProvider {}
trait SharedResource {}
trait MemoryInitiator {}
trait MemoryTarget {}
trait IpcEndpoint {}
trait ProcessingNode {}
trait CanEndpoint {}
trait EthernetEndpoint {}
```

Nodeの分類は継承ではなくCapabilityの組み合わせで表す。

## 10. Port Model

```rust
struct Port {
    id: PortId,
    name: String,
    direction: Direction,
    kind: PortKind,
}
```

```rust
enum Direction {
    Input,
    Output,
}
```

```rust
enum PortKind {
    CanTx,
    CanRx,
    EthernetTx,
    EthernetRx,
    MemoryRequest,
    MemoryResponse,
    IpcTx,
    IpcRx,
}
```

## 11. Connection Model

```rust
struct GateRef {
    node: NodeId,
    port: PortId,
}

struct Connection {
    id: ConnectionId,
    source: GateRef,
    target: GateRef,
}
```

ConnectionはTopology情報に集中し、Delay / Bandwidth / BufferはNode / Resource側へ持たせる。

## 12. Parameter Model

```rust
struct ParameterSet {
    values: std::collections::HashMap<String, ParameterValue>,
}
```

```rust
enum ParameterValue {
    Integer(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Time(SimTime),
    DataRate(DataRate),
    DataSize(DataSize),
}
```

Resolution:

```text
NED Default
   ↓ clone
INI Override
   ↓ merge
Resolved ParameterSet
   ↓
Module Factory
```

## 13. Module Registry

```rust
type ModuleFactory =
    fn(&ParameterSet) -> Result<Box<dyn NetworkNode>, ModuleCreateError>;

struct ModuleRegistry {
    factories: HashMap<String, ModuleFactory>,
}
```

Fully Qualified NED Type NameをRegistry Keyの基本とする。

## 14. Message / Transaction Model

```rust
struct MessageEnvelope {
    id: MessageId,
    created_at: SimTime,
    payload: Box<dyn SimMessage>,
}

trait SimMessage {
    fn size_bytes(&self) -> usize;
}
```

Protocol-specific型をRust-nativeで定義する。

## 15. SimContext

NodeがSimulation Engineへアクセスするための限定APIを提供する。

候補:

```rust
impl SimContext<'_> {
    fn now(&self) -> SimTime;

    fn send(
        &mut self,
        from: PortId,
        message: MessageEnvelope,
    ) -> Result<(), SendError>;

    fn schedule_after(
        &mut self,
        delay: SimTime,
        event: EventKind,
    );

    fn record_metric(
        &mut self,
        metric: MetricId,
        value: MetricValue,
    );
}
```

ModuleがSchedulerやGraph内部へ直接触れないようにする。

## 16. Buffer

```rust
enum OverflowPolicy {
    DropNewest,
    DropOldest,
    Block,
    Backpressure,
    Retry,
}
```

Queueingは汎用abstractionとしてRuntime側に持たせる。

## 17. Resource / Arbitration

共有資源共通部分を用意し、CAN Bus、AXI Interconnect、NoC、Memory Controller等へ利用する。

```rust
trait ArbitrationPolicy<T> {
    fn select(&mut self, pending: &[T]) -> Option<usize>;
}
```

実装候補:

- FixedPriority
- RoundRobin
- WeightedRoundRobin
- CreditBased

CAN Arbitrationはprotocol-specific implementationとする。

## 18. NED Parser Architecture

```text
source
  ↓
Lexer
  ↓
Token Stream
  ↓
Parser
  ↓
AST
  ↓
Semantic Validation
  ↓
Model IR
```

Parserは公開NED仕様をもとに独立実装する。

## 19. AST vs Model IR

ASTはNED構文構造を保持する。

Model IRはRuntime向けに正規化する。

```text
AST
- package declaration
- module declarations
- textual type references
- parameter expressions

Model IR
- resolved type names
- explicit Node definitions
- explicit Port definitions
- validated Connections
- resolved default ParameterSet
```

RuntimeはASTではなくModel IRを使う。

## 20. INI Resolver

INI parserは、

```text
InstancePath
ParameterName
OverrideValue
```

を出力し、Resolved Model生成時に適用する。

## 21. Metrics / Result Exporter

```rust
struct MetricSample {
    time: SimTime,
    source: NodeId,
    metric: MetricId,
    value: MetricValue,
}
```

Exporterは分離する。

```text
CsvExporter
JsonExporter
ParquetExporter
FutureVecExporter
FutureScaExporter
```

## 22. Protocol Crates

```text
sim-can
├── CanFrame
├── CanController
├── CanBus
└── CanArbitration

sim-ethernet
├── EthernetFrame
├── EthernetLink
└── EthernetSwitch

sim-soc
├── CpuCore
├── DmaEngine
├── AxiInterconnect
├── MemoryController
├── MemoryRequest
└── MemoryResponse
```

各crateは `register_modules()` でModule Registryへ登録する構成を検討する。

## 23. Runtime Flow

```text
1. Load NED
2. Parse to AST
3. Validate
4. Resolve to Model IR
5. Load INI
6. Apply Parameter Overrides
7. Create Resolved Model
8. Resolve Type Names via Module Registry
9. Instantiate NetworkNodes
10. Validate Ports / Connections
11. initialize()
12. Insert initial Events
13. Run Event Loop
14. Record Metrics
15. Export Results
```

## 24. v0.1 Implementation Order

```text
1. sim-core IDs / SimTime
2. Event / Scheduler
3. NetworkNodeCore / NetworkNode
4. Port / Connection
5. ParameterSet / Unit Types
6. Module Registry
7. Minimal NED Lexer
8. Minimal NED Parser
9. Model IR
10. INI Parser / Override Resolver
11. Runtime Message Delivery
12. Delay
13. Buffer
14. Shared Resource
15. Arbitration
16. Metrics
17. CSV / JSON Export
18. CAN Minimal Model
19. Ethernet Minimal Model
20. SoC Minimal Model
```

## 25. Rust Design Principles

1. クラス継承を再現しない
2. trait + compositionを使う
3. RuntimeとParserを分離する
4. ASTとRuntime IRを分離する
5. Port互換性は型安全に寄せる
6. Parameterは単位付き型にする
7. Errorは `Result` で明示する
8. Engine内部状態への直接アクセスを避ける
9. Moduleは `SimContext` 経由でEngineを操作する
10. Protocol modelはcore crateから分離する
11. Exporterは追加可能な構造にする
12. Deterministic simulationを優先する
