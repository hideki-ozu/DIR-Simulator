# 分散FRER経路詳細設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`design-distributed-frer`
文書状態：v1.2.0収録。本契約のfocused製品gate・独立レビュー済み。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="paths"></a>

```trace
{"id":"design-distributed-frer#paths","stage":"design","requirements":["DIR-REQ-0311"],"upstream":["architecture#distributed-frer"],"state":"confirmed","pending":[]}
```

`runtime/network/distributed.rs`は既存NetworkState/NetworkDeltaの子責務としてtyped flowとroute cursorを保持する。phaseはprefix、branch、suffixを区別し、通常のoffer/arbitrate/arrival/processing hookへ接続する。独自FESや実媒体の二重ownerは設けない。

flowはbranch開始、受理したrecovery reception、once-only continuation、実deliveryを別に保持する。最後のcopyがstale/ambiguous等で拒否された場合も、未完のbranchを残した偽のpending stateにしない。入力のphysical route identityはpreparedから再結合し、serialized mutable routeを正本にしない。

checkpoint version10は本placementにだけ適用する。物理transfer/reception、phase/hop、branch byte、履歴bitmap/timeout、suffixの親とdeliveryの因果関係を検証し、元のcodec1〜9は選択条件ごとに保持する。reserve失敗では複製fanoutとrecovery→suffixの双方でstateとbatchを公開しない。

出力はplacementで型を識別し、prefix/suffixをbranch byte集計へ混入させない。Viewerは実hopのactive transferを参照し、時刻を戻すとsequence、prefix受信、branch lifecycle、suffix開始、配送の未来情報を表示しない。
