# ネットワークMetricV2検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`test-network-metric-v2`
文書状態：基点からの追加実装を作業中。追加部分はv1.2.0収録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="dir-test-0133"></a>

```trace
{"id":"DIR-TEST-0133","stage":"verification","requirements":["DIR-REQ-0277","DIR-REQ-0278","DIR-REQ-0279"],"upstream":["design-network-metric-v2#catalog"],"state":"confirmed","pending":[]}
```

`tests/network_metric_v2.rs`は7つの実sampleについて、元のV1点とV2点のtarget/time/value/件数を照合する。別の逐次積分でqueue areaを計算し、内部u128積分器と一致させる。初期gauge、H=0、無標本、既定schema2、invalid版、専用engine拒否、target上限と観測上限、未commit prefixを確認する。pause中の二度exportで積分器が変わらず、保存・復元後の全MetricStateV2が連続実行と一致することを確認する。

`tests/network_metric_v2_viewer_model.test.cjs`は実CLIで`tas-metrics.ini`を実行し、schema3・3descriptor・全target/初期gaugeを検査する。JSONとsummary.csvの同じtarget/窓/値を照合し、Viewer loaderとstandalone HTML出力、時刻の前後移動を確認する。number値のCSV表記0.0とJSON数値0は数値で照合し、integer値は十進文字列のまま照合する。

2026-10-21確認：全7profileの観測・積分、H=0/default、pause/export/checkpoint、上限失敗のRust4件と、実JSON/CSV/ViewerのNode1件が合格。保存codecの追加整合性検査は別の保存検証記録で管理する。証跡は`/tmp/dir-remaining-implementation-20261021-evidence/integrated-metrics-fd-registry.log と network-metric-v2-viewer.log`。最終合否は統合検証記録へ反映する。
