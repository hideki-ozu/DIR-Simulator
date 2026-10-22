# Gateway冗長TSN複合詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-gateway-frer-composition`
文書状態：未commit・未公開。支持範囲の実装・製品検証・独立レビュー済み。

<a id="pipeline"></a>

```trace
{"id":"design-gateway-frer-composition#pipeline","stage":"design","requirements":["DIR-REQ-0318"],"upstream":["architecture#gateway-frer-composition"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-22` | CAN由来の複合冗長配送の採用契約と受入gate |

PreparedNetwork、BridgeState、TsnState、NetworkDeltaと共通FESを再使用する。実BridgeAction::InjectEthernetがconversion_index/branch_indexとchildを渡し、Frame.gateway_injectionがimmutableなtyped selectorと対応する。source sequenceは実prefix受信で割り当て、変換拒否・backpressure・transaction失敗では消費しない。

source SOF通知は実媒体開始の一度だけとし、terminal通知は受理済みsuffixの実endpoint受信後に行う。checkpoint private/outer16はGateway selectorがある場合だけ選択し、従来のgenerator構成を別版へ黙って読み替えない。復元では実CAN→変換→branch/child→Frame→prefix→tagged branches→recovery round→suffix→terminalという既存owner間の参照を照合する。

atomic失敗時にはGatewayの予約、Frame、queue、sequence、packetとindicationを一括して破棄する。MACの保存creditを正本にせず、既存の実packet/cause journalからの再計算を保持する。

変換MACは68B、R-TAG枝は74Bであり、Clause99の合法な分割下限124Bを下回る。変換枝は実pMACの単一packetを通る。mCRC・中断・最終fragment喪失の検証は、同じFESの逆方向枝にある実native競合frame（payload166B）による。変換枝自体の分割や同じ順方向serializer上での中断を主張しない。両方の変換枝の最終pMAC packet喪失では、recoveryとterminal受信が生成されないことを別に確認した。
