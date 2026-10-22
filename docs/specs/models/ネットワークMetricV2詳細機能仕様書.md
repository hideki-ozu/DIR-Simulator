# ネットワークMetricV2詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`spec-network-metric-v2`
文書状態：基点からの追加実装を作業中。追加部分はv1.2.0収録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="catalog"></a>

```trace
{"id":"spec-network-metric-v2#catalog","stage":"spec","requirements":["DIR-REQ-0277","DIR-REQ-0278","DIR-REQ-0279"],"upstream":["DIR-FUNC-0073"],"state":"confirmed","pending":[]}
```

対象はdynamic、TSN、CAN↔Ethernet、Gateway+TSN、抽象冗長、抽象preemption、experimental wireの7つのnetwork profile。INI `[General] metrics-version = 2`又は実行開始前の`runtime::network_metrics::enable(&mut prepared)`で選択する。既定値1は従来のschema 2を保持し、選択時は同じprofileのschema 3を出力する。1/2以外を準備時に拒否する。専用legacy engineへの指定は対応外の診断とする。

| 指標 | sampling / aggregation | 対象・意味 |
| --- | --- | --- |
| `network.queue.frames` | gauge / time_mean | 各送出portの`.queue`と`.queue.0`～`.queue.7`、bridge内CAN Controllerの`.txQueue`。既存queue_lengthと同じ待機frame数 |
| `network.queue.bytes` | gauge / time_mean | 各送出portの`.queue.0`～`.queue.7`。既存queue_bytesと同じ待機byte数 |
| `network.delivery.delay_ps` | point / mean | Ethernet endpoint。既存ethernet.delivery_psと同じ実完了配送の遅延。冗長除去で実受理した配送を含む |

targetは検証済みtopologyから決定し、未宣言の文字列を受理しない。gaugeの初期値は全対象0。送信中・中断中frameの占有を待機queueへ足さない。CANアプリCPUサービスや未完了予定配送を本配送標本へ混入しない。標本のないmeanと、H=0のtime_meanはnullとする。

既存V1観測と同じcallback deltaからV2観測を一回だけ作成し、state・timer・journalと共にpreflight後にcommitする。観測数・target数の上限超過、演算overflow、allocation失敗時は未commitのdeltaを捨てる。V1点列は保持する。整数area/windowは既存Metric V2のchecked u128で計算し、同時刻gaugeの最後の値が次区間へ適用される。

正常H=0では初期gauge catalogを保持するが、queue点列を生成しない。失敗・pauseによるH=0 prefixは成功済み初期観測を保持する。export/表示用の積分はclone上で行い、繰返しexportが実行中の積分器を進めない。

schema 3のmetadataへdescriptor、target、初期gauge、上限を保存し、JSON/CSVへ同じV2点と集計を保存する。既存V1の結果も保持する。Viewerは宣言catalogと実績を読む。利用例は`examples/ethernet/tsn/tas-metrics.ini`。

APIだけで有効化した場合、restoreには有効化後の同一Registryを渡す。INIで選択した場合は保存入力から同じcatalogを再構築してから厳密なRegistry照合を行う。入力・binary・codec版の不一致を許容する機能ではない。
