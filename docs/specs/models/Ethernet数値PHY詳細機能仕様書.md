# Ethernet数値PHY詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-numerical-phy`
文書状態：未commit・未公開。project数値契約の検証範囲を示す。

<a id="samples"></a>

```trace
{"id":"spec-numerical-phy#samples","stage":"spec","requirements":["DIR-REQ-0303"],"upstream":["DIR-FUNC-0084"],"state":"confirmed","pending":[]}
```


### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 実sample/byteと有限数値契約の証拠を規定 |

本書は`ethernet.phy.v1` / `ethernet.SampledPhy`のうち、`dir.sampled-manchester.v1`、`project.numerical.v1`の有限数値契約を規定する。IEEE選択契約は[専用仕様](IEEE選択PHY詳細機能仕様書.md)を参照する。実MAC byteからsampleを作り、受信結果を予定EOFや既知payloadから補わない。精度包絡、実carrier/collision/IFG、delay、noise、drift、有限資源とsource-gated IEEE範囲は詳細設計を正本とする。
