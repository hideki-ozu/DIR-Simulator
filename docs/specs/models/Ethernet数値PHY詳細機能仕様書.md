# Ethernet数値PHY詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`spec-numerical-phy`
文書状態：v1.2.0収録。project数値契約の検証範囲を示す。

<a id="samples"></a>

```trace
{"id":"spec-numerical-phy#samples","stage":"spec","requirements":["DIR-REQ-0303"],"upstream":["DIR-FUNC-0084"],"state":"confirmed","pending":[]}
```


### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

本書は`ethernet.phy.v1` / `ethernet.SampledPhy`のうち、`dir.sampled-manchester.v1`、`project.numerical.v1`の有限数値契約を規定する。IEEE選択契約は[専用仕様](IEEE選択PHY詳細機能仕様書.md)を参照する。実MAC byteからsampleを作り、受信結果を予定EOFや既知payloadから補わない。精度包絡、実carrier/collision/IFG、delay、noise、drift、有限資源とsource-gated IEEE範囲は詳細設計を正本とする。
