# AHBパイプライン詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
公開版：未公開。タグ・Releaseは作成していない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | Full AHB2の実行pipeline・burst・lock・SPLIT/RETRYと版付き状態保存を追加。初版予定値 |

文書ID：`design-ahb-pipeline`

文書状態：対象commitは基点であり、本追加は未commit。未公開の実装契約。追加要件DIR-REQ-0286・機能DIR-FUNC-0076の担当範囲を定め、旧profileの契約を変更しない。

<a id="pipeline"></a>

```trace
{
  "id": "design-ahb-pipeline#pipeline",
  "stage": "design",
  "requirements": ["DIR-REQ-0286"],
  "upstream": [
    "architecture#ahb-pipeline"
  ],
  "state": "confirmed",
  "pending": []
}
```

The family reuses the existing Registry/Model/Context execution owner. `runtime/ahb_pipeline/mod.rs` nests its strict input module and public schema types using path declarations; only runtime module/default-registry registration needs central integration. `ahb.Pipeline` owns one actual bus, declared manager priorities and decoded RAMs. `ahb.pipeline.Tick` is an empty phase1 timer; one successful callback advances one active cycle.

The state has separate `address` and `data` slots. An address slot retains request, beat, attempt, HTRANS and effective HBURST. Its payload/control never changes during HREADY wait. A data slot owns the earlier accepted address, remaining ordinary wait cycles, eventual response, two-cycle response position and optional HSPLIT release delay. This preserves the data master while a different manager owns the next address. The workload supplies immutable write bus lanes; MemoryService is called only on OKAY completion.

The cycle first emits due HSPLIT indications and updates only the associated split masks. If no address is held, fixed-priority arbitration selects each manager's oldest unfinished workload request, subject to mask/lock restrictions. A new lock spends a setup cycle before address presentation. Current data then either waits, produces the first non-OKAY response, or completes. A successful data completion writes/reads real bytes. A failed attempt never writes. First non-OKAY response cancels the speculative address, and the second response retains IDLE. SPLIT/RETRY restore the failed beat as the manager's next beat with a new attempt; completed earlier beats are retained.

When HREADY permits it and no cancellation occurs, the accepted address becomes the next data slot. Unlocked fixed-priority preemption fragments the interrupted burst into legal remaining SINGLE/NONSEQ beats, avoiding a false claim that a resumed WRAP4 is still a complete four-beat sequence. A locked burst retains ownership even while its final response is unresolved. Locked SPLIT reserves only the dummy/default IDLE path until HSPLIT. The source specification and the official example-system arbiter state diagram establish this behavior; the design does not infer it from later AHB-Lite rules.

`transaction_fabric::memory::MemoryService` owns the sole byte array for the bus's targets. HSIZE/address derive contiguous enabled bytes from actual bus lanes; reads pack the response into those same lanes. This service is shared implementation code with the concurrent AXI profile, not a shared runtime array between separately executed profiles. Composing AXI and AHB into one live system still requires an explicit common owner/adapter and is not claimed here.

Each callback works on private cloned state and stages records/timers through the existing effect batch. All time arithmetic and attempt/record increments are checked before publishing the model state. The codec compares parsed immutable config/workload, validates map/counters, separate slot positions, response-rule consistency, lock owner, mask source/attempt/release bounds and live timer references. The generic bundle retains pending events and prior observations. Sample stop/checkpoint tests inspect partial writes and compare resumed records/memory exactly to uninterrupted execution.
