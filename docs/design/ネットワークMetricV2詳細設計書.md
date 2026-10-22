# ネットワークMetricV2詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-network-metric-v2`
文書状態：基点からの追加実装を作業中。追加部分は未commit・未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。prepared catalog、所有delta、共通積分器と出力の責務を記録 |

<a id="catalog"></a>

```trace
{"id":"design-network-metric-v2#catalog","stage":"design","requirements":["DIR-REQ-0277","DIR-REQ-0278","DIR-REQ-0279"],"upstream":["architecture#network-metric-v2"],"state":"confirmed","pending":[]}
```

[仕様](../specs/models/ネットワークMetricV2詳細機能仕様書.md#catalog)のcatalogは`runtime/network_metrics.rs`が検証済みnetwork topologyから生成する。preparedとRegistryをcloneした候補だけを変更し、衝突・上限・再現情報の検査が成功した後に公開する。再度enableしても同じcatalogを重複登録しない。通常の`Context`による任意Modelのtarget所有制約は変更しない。

networkの内部ownerは、自身の`NetworkDelta.points`だけを`stage`へ渡す。stageは既存観測名を版付きV2名へ対応付け、declared targetへの`Effect::MetricV2`を追加する。`registered::network_step`はstate予約と既存`MetricStateV2::preflight`を行い、元の一つのcommit経路で適用する。`NetworkArena::prepare`でV2を再適用しない。

catalogはprepared Registryの一部であり、checkpointのRegistry fingerprintへ含まれる。live積分器は従来のcore codecで保存し、network state codecと同じcommit境界に属する。APIだけのcatalogをdefault Registryに暗黙追加しない。INI選択についてだけ、built-in provenanceと保存済み入力から決まるcatalogを再構築する。

出力は`output/registered.rs::metric_catalog_v2`を汎用／network metadataで共有する。V1/V2のJSON・CSV投影と、schema 3 Viewerの検算を実結果で照合する。gauge areaの積分器、半開窓、u128 decimal codecを新実装へ複製しない。
