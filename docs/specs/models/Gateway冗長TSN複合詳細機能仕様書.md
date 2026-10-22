# Gateway冗長TSN複合詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-gateway-frer-composition`
文書状態：未commit・未公開。支持範囲の実装・製品検証・独立レビュー済み。

<a id="pipeline"></a>

```trace
{"id":"spec-gateway-frer-composition#pipeline","stage":"spec","requirements":["DIR-REQ-0318"],"upstream":["DIR-FUNC-0093"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-22` | CAN由来の複合冗長配送の採用契約と受入gate |

実CAN転送から採用ruleのEthernet egressを作り、一つのordinary prefixを経てbridge-pairの複製点へ到達させる。複製枝はR-TAG付き実bytesをそれぞれCBS/TASとClause99に渡し、受理したrecoveryからsuffixを実送信する。source selectorはGenerator又はGatewayEgress(gateway_id,rule_id,egress_index)である。legacy source_generatorはGeneratorへ変換し、Gateway由来を架空のgeneratorへ読み替えない。

元CAN生成、変換、Ethernet受付、実SOF、recovery、実宛先受信を区別する。unique targetの完了summaryは最初の実受信で確定し、規範timeout後の追加受理は別の実suffix受信indicationとして残す。予定EOFやelimination到着から最終宛先成功を合成しない。

初期の支持構成は一つのordinary prefix、bridge-pair配置、規範vector recovery、静的ideal-clock CBS/TASと実Clause99枝である。prefixそのものの同時fanoutや未結合adapterを支持済みと解釈しない。

変換MACは68B、R-TAG枝は74Bであり、Clause99の合法な分割下限124Bを下回る。変換枝は実pMACの単一packetを通る。mCRC・中断・最終fragment喪失の検証は、同じFESの逆方向枝にある実native競合frame（payload166B）による。変換枝自体の分割や同じ順方向serializer上での中断を主張しない。両方の変換枝の最終pMAC packet喪失では、recoveryとterminal受信が生成されないことを別に確認した。
