# TSNローカル時計詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-local-tsn-clock`
文書状態：未commit・未公開、製品gate実施中。

<a id="clock"></a>

```trace
{"id":"spec-local-tsn-clock#clock","stage":"spec","requirements":["DIR-REQ-0309","DIR-REQ-0310"],"upstream":["DIR-FUNC-0087"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 独立node時計とTAS/受信PSFPの結合契約 |

## 時刻と受入契約

単調なglobal時刻をFES、MAC送信、IFG、物理伝搬とmeterの基準とし、nodeごとの有理数drift・signed offset・step/slewからlocal時刻を求める。浮動小数点を使わず、負のlocal時刻とoverflowを定義済みの整数射影で処理する。既存ideal_shared profileの意味は保持する。

明示的なnetwork compositeのTASは送信node、PSFPは受信nodeのlocal時刻でgateを判定する。補正は未送信TAS wakeを再評価し、同じglobal時刻でもlocal gateの開閉を変える。PSFP gateだけがlocal時刻を読む。帯域meterは実経過global時刻で補充し、時計step/slewだけでtokenを増やさない。

`examples/network-composite/psfp-clock`は同じ2つの送信をidentity、+500000ps offset、前後stepで比較する。実到着576000/2576000ps、送信0/2000000psは全variantで同一であり、offsetでは2件、stepでは後半1件のみ通過する。受信成功を予定時刻から合成しない。

本契約は設定された時計と補正を実TSNへ結合する。gPTP packetの選出・同期・peer-delayによる補正生成は別の未完能力T02である。未結合のCBS/GCL-update/PHY組合せを黙認せず、個別adapter受入後に開く。
