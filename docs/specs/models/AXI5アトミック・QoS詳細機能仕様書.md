# AXI5アトミック・QoS詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
公開状態：v1.2.0収録。実装台帳に記録した契約・検証範囲を参照。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

文書ID：`spec-axi5`
文書状態：実装対応草案。受け入れ契約と実行済み証跡を区別する。

<a id="atomic"></a>

```trace
{"id":"spec-axi5#atomic","stage":"spec","requirements":["DIR-REQ-0300","DIR-REQ-0301"],"upstream":["DIR-FUNC-0082"],"state":"confirmed","pending":[]}
```

## AXI5 extension

`soc.execution.v1`の`operation:atomic`は明示AXI5 extensionであり、AXI4の機能と表現しない。一次根拠はARM IHI0022H E1-345/346。32bit little-endian aligned operandだけを受け付ける。AWATOP `0x10..0x17`はAtomicStore、`0x20..0x27`はAtomicLoad、low3bit順はADD/CLR/EOR/SET/SMAX/SMIN/UMAX/UMIN。`0x30`はSwap、`0x31`はCompare。big-endianと予約opcodeは拒否する。

Store/Load/Swapは4byte outbound、Compareは8byte。Compareのtargetが8byte窓下半分ならcompare→swapとINCR、上半分ならswap→compareとWRAPを要求する。compare不一致時は元値を保持する。Load/Swap/Compareは更新前4byteをRで返し、StoreはBだけを返す。R/Bのbackpressureは独立で、directory exclusionを実完了まで保持する。全操作はwrap算術、signed min/max境界を定義する。

## QoSと有限仲裁

全sourceのrequestは4bit `qos` 0..15を持つ。有限global queueへsource rotating admissionを使い、特定sourceの連続投入で他sourceを常時拒否しない。owner arbitrationのeffective値は`min(15,qos+wait_cycles/aging_cycles)`、tieはoldest/source/ID/sequence順。同じsource/IDの先行操作を追い越さない。recordはbase/effective、待機量、starvation counterとgrant理由を記録する。無限の高QoS到着下ではbounded agingと公平admissionを前提とし、実機全interconnectの性能上限を主張しない。

AXI4 native adapterのAWQOS/ARQOSにも実4bit値を記録する。AXI5 atomic区間はnative ordinary AXIをdrainしてburst ownerを予約する。AWとWは独立に進められるが、W burstは途中で他requestへ切り替えない。
