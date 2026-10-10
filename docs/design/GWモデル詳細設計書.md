# GWモデル詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 2f1e60b`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | GWの公開Registry接続、専用エンジン・schema2結果・系譜の配置と、外部登録モデルの検証境界を反映 |
| `1.0.0` | `2026-10-03` | RX保持、出力別のTX受理待ち、分岐、コピー・出典管理と実装配置を反映。文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。独立CANバス・静的GWの契約と検証を具体化 |

文書ID：`design-gw`

| 項目 | 内容 |
| --- | --- |
| 状態 | 設計確定。RX保持・TX満杯待ちを内部実装し、[改訂後の実行記録](../verification/results/gateway-rx-buffer-2026-10-03.json)で改訂fixtureと境界・分岐・順序・終了/表示を確認した。従来16 fixture照合の証跡とは取得時のソースを区別する。公開Registry/Envelope/Context APIを開発中ソースへ追加。GWは専用エンジンの組込みアダプターとして動作し、外部登録GWの製品適合は未確認 |
| 担当元 | [architecture#arch-gateway](../アーキテクチャ設計書.md#arch-gateway)。GW coordinatorと既存CAN contextを結合する |
| 正本 | [GW仕様](../specs/models/GWモデル詳細機能仕様書.md)と[CAN設計](CANモデル詳細設計書.md)。共通scheduler・journal・結果のcommit境界を再利用する |

現行ソースでは、[types.rs](../../crates/dir-simulator/src/lib/types.rs)のPreparedSimulationの`can`へバス・Controller所属、`gateway`へGateway・Routeを格納し、[input/gateway.rs](../../crates/dir-simulator/src/input/gateway.rs)でJSON、所有区間と閉路を検証する。[runtime/gateway.rs](../../crates/dir-simulator/src/runtime/gateway.rs)はEngineが所有する一つのschedulerにバス別active/dirty状態を持ち、受信完了→経路照合→GW処理→コピー生成→CAN Readyを実行する。[snapshot.rs](../../crates/dir-simulator/src/lib/snapshot.rs)は`can`に確定したRequest/Receiver、`gateway`にForwardと要求ID別の系譜、`common`に計測点・終了状態を保持し、[output.rs](../../crates/dir-simulator/src/output.rs)がschema2へ変換する。これはGWの専用実装であり、公開Registryには組込みアダプターとして接続する。外部登録モデル用のRegistry/Envelope/Contextは別の共通エンジンで提供する。

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
| GatewayState | node、portsのソート済配列、routesを(ingress,format,min,id)順で保存、processing_delay、hop_limit、rx_queue_capacity、forward行Map、RX保持行Map、ingress別保持数、egress別waiting_txの初回ready順、pending token Map。RXは親ごとに全枝の未受理・未終端数を保持する。初期stateへrx_buffers:[]を追加し、forward/RX保持/待ち/timerは空、保持数0。metadata.initial_stateはcoordinator内部IDへ初期state、構造moduleは従来どおり`{}`を対応付ける。Controller/Bus/channelは既存初期stateのprofileだけ選択profileにする |
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
| RouteInput、1 | ingressのRX保持数が容量以上ならrx_queue_full行とGW破棄計測だけをcommitし、CAN成功・受信行を保持する。受理ならRX一枠を確保し、一致なしはfiltered行とRX即解放をcommit。一致時はegress順にcopy ID・origin・hop+1・予定時刻を計算しprocessing行を作成、正delayの完了timer又は0delayのForwardを予約。全複製の算術と予約を検査後、一つの効果batchで公開する |
| ForwardDue、0 | processing行とtokenの一致を検査し、同時刻phase1のForwardを予約。forwarded_psはまだnull。0delay経路はこの完了timerを省略する |
| Forward、1 | hop加算はchecked u32で計算し0..65536に制限してhop上限を確認。破棄は転送行を終端しRXの該当枝を完了。適合ならRequestとMultibusFieldsを一回だけ構成しforward行submittedへ更新。RequestはegressのCAN contextへ局所deltaとして登録し、既存TX ready処理を一回だけ予約する。RX枠は保持する。state更新と全効果を同一batchでcommit |
| CanTxRequest、1 | ready_psをTX処理完了として確定。容量0ならqueue_fullで枝終端。正容量で空きありならpending・tx_enqueued_ps=現在時刻とTX待ち計測をcommitし枝完了。満杯ならwaiting_txと初回ready event順を保持する。waiting_txはRX枠に属する状態であり、CAN TXキューへ二重計数しない。Bus BUSYは受理判定の条件にしない |
| TX空き通知、次deltaの1 | ArbitrateのSOFでキューから要求が取り除かれたらegress単位に一つの起床を予約し、全ingressのwaiting_txを初回ready event順に空き数まで受理する。途中から来たreadyを先行待ちより優先しない。再試行でRequest/ID/timerを作り直さず遅延も加算し直さない。GW portの仲裁同値tie keyは受理順とする |
| RX解放、1 | 全egressが受理又はhop/容量0で終端した親だけRX行をreleasedへ更新し、ingress保持数を一回減らしてRX長を記録する。分岐ごとの完了状態を保持し、待っていない枝は独立に進める |
| Arbitrate、2 | dirty Busを正規パス順に評価。勝者のframe_bitsと当該bitrateで予定EOF/releaseを計算する。同時刻の別Bus SOFを別batchでcommitできる。転送中の既存フレームは非プリエンプティブ。SOFのキュー除去を空き通知の起点とする |
| EOF→次GW | 通常の同報Rxにorigin/parent/hopsを付けて配送。送信したport自身にはReceiverを作らず、同Busの他GW portは通常の受信者として処理する |
| failure | 重複routing・unknown parent・frame/origin/時刻不一致・token不一致はE-0002/model_failed。効果commit失敗は直前journal prefixからsnapshotを構成し、モデルを再開する処理は対象外（注） |
| finish | [0,T)後のsnapshotにprocessing forward行、RX holding、TX処理中・waiting_tx・pendingを含む全Bus/要求状態を残し、その後token・shared frameを解放する。copy未作成と未送信を区別し、T時点のForward・空き通知・RX解放を実行する処理は対象外（注） |

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
| snapshot/export | Request・Receiver・forward・RX保持行を共通ModelRecord envelopeに格納し(schema_name,subject,record_id)順で出力。gw.forwardの既存形は保持し、gw.rx_buffer/1とschema2のnullable tx_enqueued_psを検証する。モデル固有行のschemaはGW pluginが検証、共通exporterは登録kind/versionのpayloadを運搬する。Requestからsubmittedコピーの最終結果、RX行から親ごとの保持区間を再構成できることを参照整合検査する。両schemaのmetadata.topology.controllersへ準備済みBus所属・TX/RX経路遅延を出力する |
| 計測 | Busごとのbusy時間を個別積分し、他Busの重なった区間を引かない。GW共通Recordはspecの増分点と終了集計を生成。RX長・最大・rx_queue_fullとTX受理時のtx_enqueued−readyを追加し、RX破棄をCAN droppedへ混ぜない。forward_id→child→parent→originとRX→parent→originが同run内に実在することを確認し、origin別集計は全体母数と分ける |
| viewer復元 | topologyを正確なController–Bus接続の正本とし、正規化routesをController間の論理経路として重ねる。RX holdingはdropped行を除きreceived≤選択時刻かつreleased未到達の親行から求め、waiting_txはready到達/tx_enqueued未到達で復元する。巻き戻しは台帳から再計算し、描画上限・残像を状態件数へ算入しない |
| 回帰 | 既存profileは既存decoder/serializer/Controller tie keyをそのまま選択。schema1へGW追加キーを混入させる処理は対象外（注） |

[検証仕様](../verification/cases/GWモデル検証仕様書.md)の0011～0016で構成・時間・容量・範囲・停止・回帰を分担する。
