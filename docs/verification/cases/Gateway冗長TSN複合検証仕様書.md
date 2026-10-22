# Gateway冗長TSN複合検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`verification-gateway-frer-composition`
文書状態：未commit・未公開。支持範囲の実装・製品検証・独立レビュー済み。

<a id="dir-test-0151"></a>

```trace
{"id":"DIR-TEST-0151","stage":"verification","requirements":["DIR-REQ-0318"],"upstream":["design-gateway-frer-composition#pipeline"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-22` | CAN由来の複合冗長配送の採用契約と受入gate |

必須gateは実CAN bit trace、変換payload/VID/PCP、R-TAG/FCS/mCRC、解析的なshaped SOF/credit、復元payloadと元CANのend-to-end遅延である。一枝喪失、最終fragment喪失、変換拒否、backpressure、途中horizon、timeout後の実追加配送を確認する。

変換前、queued prefix、実source SOF、suspended fragment、recovery、suffix flightで別processの保存継続を比較する。selector、conversion/branch、child、byte/VID/PCP、SOF、terminalとroundのrehash改変を拒否し、reserve失敗時にchild/sequenceが漏れないことを確認する。既存generator FRERとCAN compositeを回帰する。

実施結果は製品13件、実callbackでのatomic rollback1件（変換注入とprefix受信後の2cut）、既存76件が合格。別processの7保存cutとrehash改変13種を含む。独立レビューで製品13件とatomic1件を再実行した。実CLIの元CAN要求から実宛先まで501,930,000ps、二つの実CAN要求のCBS SOFは500us/503.008usである。Viewerは実受信前の行を表示せず、round別受信と逆再生を確認した。PC/mobileで4つの実結果を検証した。[独立レビューと実施記録](../results/remaining-20261021/gateway-frer-final-review.md)を参照。

変換MACは68B、R-TAG枝は74Bであり、Clause99の合法な分割下限124Bを下回る。変換枝は実pMACの単一packetを通る。mCRC・中断・最終fragment喪失の検証は、同じFESの逆方向枝にある実native競合frame（payload166B）による。変換枝自体の分割や同じ順方向serializer上での中断を主張しない。両方の変換枝の最終pMAC packet喪失では、recoveryとterminal受信が生成されないことを別に確認した。
