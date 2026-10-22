# 分散FRER経路詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`spec-distributed-frer`
文書状態：v1.2.0収録。本契約のfocused製品gate・独立レビュー済み。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="paths"></a>

```trace
{"id":"spec-distributed-frer#paths","stage":"spec","requirements":["DIR-REQ-0311"],"upstream":["DIR-FUNC-0088"],"state":"confirmed","pending":[]}
```

`ethernet.frer.rtag.v1`の`placement.kind=bridge_pair_v1`は、明示した複製bridgeと除去bridgeの間を実物理経路で結ぶ追加契約である。prefix、2つ以上のbranch、suffixを連続したport列で設定する。各copyは通常の有限queue、競合、link障害、送信と伝搬を通る。同一出力を共有するbranchも、そのqueueで直列化する。

prefixとsuffixは通常MACを運び、branchだけが実R-TAGと再計算FCSを運ぶ。除去点では実受信したtag、sequence、FCSを検証してからtagを外す。受理したcopyがsuffixを開始することと、実宛先に到着して処理を完了することを別に記録する。除去bridgeの到着を宛先配送に読み替えない。

この契約の既定recoveryはプロジェクト定義の有限履歴である。originごとのsuffix開始は一回に制限し、suffixの喪失後に遅れて別copyが到着しても、完了を合成したりsuffixを再生成したりしない。IEEE 802.1CB vector recoveryの全規範、特にtimeout後の再受理は別のN02契約である。

copyごとのTAS/PSFPを既存経路で適用し、制御はLinkSetを対象とする。未結合のprovider、Qcc、wire control、能動multicast、共有LAN、PHY、preemption、CAN bridgeの同時指定は具体的な入力診断で拒否する。既存のplacement省略入力と抽象profileはそのまま保持する。
