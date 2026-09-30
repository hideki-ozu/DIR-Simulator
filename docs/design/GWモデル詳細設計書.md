# GWモデル詳細設計書

文書バージョン：`0.1.0`
対象GitHubバージョン：`未リリース（main @ 7738b55）`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。独立CANバス・静的GWの契約と検証を具体化 |

文書ID：`design-gw`

| 項目 | 内容 |
| --- | --- |
| 状態 | 設計確定。実装・製品試験は未実施 |
| 担当元 | [architecture#arch-gateway](../アーキテクチャ設計書.md#arch-gateway)。GW coordinatorと既存CAN contextを結合する |
| 正本 | [GW仕様](../specs/models/GWモデル詳細機能仕様書.md)と[CAN設計](CANモデル詳細設計書.md)。共通scheduler・journal・結果のcommit境界を再利用する |

<a id="validation"></a>

## 1. 静的構成と区間検証

```trace
{
  "id": "design-gw#validation",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0124",
    "DIR-REQ-0125",
    "DIR-REQ-0129",
    "DIR-REQ-0130"
  ],
  "upstream": [
    "architecture#arch-gateway"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 内部要素 | データ・アルゴリズム |
| --- | --- |
| MultibusRegistry | `BusId→BusContext`、`ControllerId→BusId`、`GatewayId→GatewayState`、`ControllerId→GatewayId?`を保持。BusContextは自バスだけのRequest/候補/占有を変更する |
| 構築順 | 全simple/channelの構築後にmodule設定を解決し、内部ID`@profile:can.cc.multibus.v1:<node>`のcoordinatorを生成する。全モデルのinitialize前に生成を完了し、共通ライフサイクル・依存順・破棄順へ参加させる。新しい型キーは固定multibus descriptorを持ち、INI解決後のdescriptor変更は対象外（注） |
| GatewayState | node、portsのソート済配列、routesを(ingress,format,min,id)順で保存、processing_delay、hop_limit、forward行Map、pending token Map。coordinatorの初期状態は`{profile:"can.cc.multibus.v1",forward_records:[],pending:[]}`。metadata.initial_stateはcoordinator内部IDへこのstate、構造moduleは従来どおり`{}`を対応付ける。Controller/Bus/channelは既存初期stateのprofileだけ選択profileにする |
| Schema検証 | [configuration](../specs/models/GWモデル詳細機能仕様書.md#configuration)の許可field集合・型・必須・defaultを適用し元JSON pointerを保持。prepare時に同じnode/portが複数GWへ登録されたら双方の位置を診断 |
| OwnerIntervalMap | formatごとにnative点とegress区間を(min,max,source)でsort。重なる異sourceの組を診断。同sourceは区間和へ正規化。u64でmax+1を計算して29bit最大端点の後端も表現する |
| 閉路検証 | 全routeのmin/max+1をformat別にsort/unique。隣接区間の先頭xを代表値としてxを包含する全routeからBus辺を生成しDFSのgray頂点への辺を閉路とする。空グラフ・重複辺は受理。経路数に依存する計算を行い、全2^29 ID列挙を避ける |
| FrameStore | native generatorのCanFrame値をkeyとする不変WireFrame共有参照。GWに到着する全frameはnative起点の参照を継承するため実行時の再計算を必要としない。元Request行の寿命とframeメモリの寿命を分離し、全コピー・イベント参照が終了するまで保持する |
| 境界 | CAN ideal側の単一Bus制約はprofile validatorで保持。multibus側は各Busに同じ接続/bitrate/ACK前提を適用する。全準備成功後にのみ初期イベントを予約する |

<a id="routing-state"></a>

## 2. 受信から転送先の仲裁まで

```trace
{
  "id": "design-gw#routing-state",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0124",
    "DIR-REQ-0125",
    "DIR-REQ-0126",
    "DIR-REQ-0127",
    "DIR-REQ-0128",
    "DIR-REQ-0131"
  ],
  "upstream": [
    "architecture#arch-gateway"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 手順・phase | モデル変更と効果 |
| --- | --- |
| Received、1 | 既存CANがReceiver.receivedをcommitする同batchでGW受信通知を同phase1へ追加する。通知を一回だけ受理するseen(parent,ingress)集合を保持する。既存Receiverの時刻・frame参照を検証する |
| RouteInput、1 | 一致なしはfiltered行と計測をcommit。一致すればegress順にcopy ID・origin・hop+1・予定時刻を計算しprocessing行を作成、正delayの完了timer又は0delayのForwardを予約。全複製の算術と予約を検査後、一つの効果batchで公開する |
| ForwardDue、0 | processing行とtokenの一致を検査し、同時刻phase1のForwardを予約。forwarded_psはまだnull。0delay経路はこの完了timerを省略する |
| Forward、1 | hop加算はchecked u32で計算し0..65536に制限してhop上限を確認。破棄は転送行だけ終端。適合ならRequestとMultibusFieldsを構成しforward行submittedへ更新。RequestはegressのCAN contextへ局所deltaとして登録し、既存TX ready処理を予約する。state更新と全効果を同一batchでcommit |
| CanTxRequest、1 | 既存CANの容量判定を実行。GW portは仲裁同値時のtie keyをready投入sequenceとする。queueCapacityを一つのキューで管理し、coordinator側へ重複待機キューを作る処理は対象外（注） |
| Arbitrate、2 | dirty Busを正規パス順に評価。勝者のframe_bitsと当該bitrateで予定EOF/releaseを計算する。同時刻の別Bus SOFを別batchでcommitできる。転送中の既存フレームは非プリエンプティブ |
| EOF→次GW | 通常の同報Rxにorigin/parent/hopsを付けて配送。送信したport自身にはReceiverを作らず、同Busの他GW portは通常の受信者として処理する |
| failure | 重複routing・unknown parent・frame/origin/時刻不一致・token不一致はE-0002/model_failed。効果commit失敗は直前journal prefixからsnapshotを構成し、モデルを再開する処理は対象外（注） |
| finish | [0,T)後のsnapshotにprocessing forward行と全Bus状態を残し、その後token・shared frameを解放する。copy未作成と未送信を区別し、T時点のForwardを実行する処理は対象外（注） |

<a id="payload-export"></a>

## 3. payloadと計測の組立

```trace
{
  "id": "design-gw#payload-export",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0124",
    "DIR-REQ-0126",
    "DIR-REQ-0127",
    "DIR-REQ-0128",
    "DIR-REQ-0131",
    "DIR-REQ-0132"
  ],
  "upstream": [
    "architecture#arch-gateway"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 境界・内部API | 責務 |
| --- | --- |
| decode_tx/decode_rx | [payload-results](../specs/models/GWモデル詳細機能仕様書.md#payload-results)の完全fieldと型を検査。Envelopeと意味が一致することをhandlerが検査。形式不正はinvalid_event、復号後不整合はmodel_failed |
| on_received(parent,ingress) | committed Receiverへの参照のみ受理。parent Requestと不変WireFrameを参照しGatewayDelta＋effectsを返す。sim-can→GWの直接再入callbackを避け、登録したphase1通知を介する |
| Internal RoutingInput | `{parent_request_id,ingress,received_ps}`。frame/origin/hopsは共有台帳から取得。内部ForwardDue/Forwardは`{forward_id}`。これらは内部timerであり外部ポートpayloadと別descriptor |
| on_forward(forward_id) | 参照parent、送信先、hop、タイマ時刻を照合し、Forward行とcopy Requestのdeltaを一括生成。prepared frame handleを保持しsource/bus/bitrateだけ送信先へ置き換える |
| snapshot/export | Request・Receiver・forwardを共通ModelRecord envelopeに格納し(schema_name,subject,record_id)順で出力。モデル固有行のschemaはGW pluginが検証、共通exporterは登録kind/versionのpayloadを運搬する。Requestのstateからsubmittedコピーの最終結果を再構成できることを参照整合検査する |
| 計測 | Busごとのbusy時間を個別積分し、他Busの重なった区間を引かない。GW共通Recordはspecの増分点と終了集計を生成。forward_id→child→parent→originが同run内に実在することを確認し、origin別集計は全体母数と分ける |
| 回帰 | 既存profileは既存decoder/serializer/Controller tie keyをそのまま選択。schema1へGW追加キーを混入させる処理は対象外（注） |

[検証仕様](../verification/cases/GWモデル検証仕様書.md)の0011～0016で構成・時間・容量・範囲・停止・回帰を分担する。
