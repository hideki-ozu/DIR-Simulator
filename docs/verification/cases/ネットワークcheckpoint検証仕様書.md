# ネットワークcheckpoint検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`test-network-checkpoint`
文書状態：基点からの追加実装。追加部分はv1.2.0収録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="dir-test-0130"></a>

```trace
{"id":"DIR-TEST-0130","stage":"verification","requirements":["DIR-REQ-0280","DIR-REQ-0281","DIR-REQ-0282"],"upstream":["design-network-checkpoint#state"],"state":"confirmed","pending":[]}
```

`tests/network_checkpoint.rs`で7 profile×0/1/4/10境界の28件と、CBS/gatedCBS/PSFP/GCL/FRER/中断preemptionの6稼働状態、INI Metric V2の1件、合計35件を別processへ引渡して通常実行とcanonical結果を比較した。保存後に元scenarioを書換え、FrozenInputsだけで復元できることを確認した。

汎用control/checkpoint 45件、network論理7件、u128境界を別に確認する。Completeの誤index、必須Dueの欠落、foreign schema/future time/scalar Arena record、descriptor拒否の回帰試験を含む。最後のArenaガード追加後は該当するfocused回帰を実行した。全process matrixの証拠と、最後の小修正のfocused証拠を区別する。

`CheckpointDecodeContext::timer_envelope`は登録codecへ自身の実FES envelopeを読取専用で提供する。NoCとCANのmodel内deadline/payload照合に使い、汎用token所有権だけで将来時刻が正しいと判断しない。
