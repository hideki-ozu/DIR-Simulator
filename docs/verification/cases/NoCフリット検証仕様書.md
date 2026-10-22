# NoCフリット検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
公開状態：v1.2.0収録。実装台帳に記録した契約・検証範囲を参照。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

文書ID：`verification-noc-flit`
文書状態：実装対応草案。下表は検証契約であり、実行結果は統合作業時の証跡に記録する。

<a id="dir-test-0135"></a>

```trace
{"id":"DIR-TEST-0135","stage":"verification","requirements":["DIR-REQ-0291","DIR-REQ-0292","DIR-REQ-0293"],"upstream":["design-noc-flit#network"],"state":"confirmed","pending":[]}
```

## 受け入れ群

[統合試験](../../../crates/dir-simulator/tests/noc_flit.rs)と[破損復元試験](../../../crates/dir-simulator/src/runtime/noc_flit/checkpoint.rs)を正本とする。

| 群 | 入力・期待値 | 試験入口 |
| --- | --- | --- |
| 1 credit | 1flit容量、link2cycle、credit3cycle。次flitのlaunchは5cycle後、実credit返却より早く出ない。全callbackで容量式を検査 | `finite_credit_delay_tail_ownership_and_exact_payload` |
| 2 VC保持 | 3flit packetの全flitが同じdownstream VCを使い、他headはtail離脱以前に奪えない。末尾1byte、単一head-tail、0byteを検査 | 同上、`concurrent_links_shared_link_fairness_and_control_flit` |
| 3 仲裁 | 独立linkは同時launch、全traceで同じlink/timeの二重launchなし、複数候補が有限時間で配送 | `concurrent_links_shared_link_fairness_and_control_flit`、輻輳試験 |
| 4 escape | torus輻輳の36packetがescapeへ入りdrain。headはescape→adaptiveへ戻らない。独立に全src/dst tree pathからchannel依存graphを構築し、複数mesh/torusで循環がない | `adaptive_stalls_enter_permanent_escape_and_drain`、`independent_escape_channel_dependency_graph_is_acyclic` |
| 5 torus | 幅4で0→3はtorus1hop/mesh3hop。2×2は16有向linkでparallel reverse portも別。tree pathが非最短となる実例を検査 | `torus_wrap_parallel_ports_and_tree_detours` |
| 6 故障 | arrivalと同時故障は先にabort。影響外のinjected packetもepoch時に一度abortし、未投入requestは残り経路を使う。分断はunreachable、回復後は配送 | `fault_at_arrival_aborts_once_reroutes_pending_partition_and_recovery` |
| 7 部分・継続 | mid-transit horizonはdeliveryなし。複数callback境界でcheckpointし、復元直後と完走record/payloadが無中断runと完全一致 | `full_checkpoint_resume_at_transit_credit_ejection_and_fault_boundaries` |
| 8 破損 | duplicate flit、credit leak、stale epoch、orphan owner、wrong timer、過去arrival、wrong treeの明示codec loadを拒否 | `malformed_checkpoint_rejects_duplicate_credit_epoch_owner_and_timer` |
| 9 境界 | 単一router local、0byte、pending溢れ、完全empty、packetなしfuture fault、厳格入力・同時packetの配列順非依存 | `finite_pending_admission_local_router_empty_and_future_fault`、`strict_invalid_inputs_and_order_independence` |
| 10 製品統合 | [sample](../../../examples/noc-flit/noc-flit.ini)をproduction CLIでprepare/runしschema3の実recordを確認。共通schema3 Viewer、既存noc.xy.v1、通常gateは親統合作業で確認 | CLIと共通回帰 |

focused commandは`cargo test -p dir-simulator --test noc_flit`および`cargo test -p dir-simulator --lib runtime::noc_flit`。既存`noc.xy.v1`の抽象transaction契約と、新profileの実flit契約を区別する。規格全体適合、百万packet性能、dateline方式、非対称有向routingを試験済みとはしない。
