# GWモデル詳細機能仕様書

文書バージョン：`0.1.1`
対象GitHubバージョン：`v0.1`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。独立CANバス・静的GWの契約と検証を具体化 |

文書ID：`spec-gw-models`

| 項目 | 内容 |
| --- | --- |
| 状態 | 仕様決定済み。製品実装・製品試験は未実施 |
| 適用 | `can.cc.multibus.v1`。独立した理想CAN CCバスとstore-and-forward GW。既存[CAN仕様](CANモデル詳細機能仕様書.md)のframe・CRC・仲裁・ACKを再利用する |
| 境界 | 追加profileを明示選択する入力を扱う。既定`can.cc.ideal.v1`とschema1の単一バス挙動は既存仕様を適用する。注：物理層・CAN規格全体への適合認証、ID変換、動的経路、通信エラーは対象外 |

## 元通信とコピーの確認単位

[独立CANバスとGW転送](../../要件定義書.md#dir-req-0123)は、[追加モデルの選択と評価](../../要件定義書.md#dir-req-0157)のモデル別分担であり、1.0.0のGW・複数CANに向けた仕様である。契約の確定状態と提供・実装の完了状態は区別する。

| 確認段階 | 本書の担当節 | 親要件に対する連携確認 |
| --- | --- | --- |
| バスと経路の準備 | [configuration](#configuration) | 各BusのController対・bitrateを確定し、所有区間とID区間別循環を検査する。独立Busの同時SOFを受理できても、転送経路が不正なら全体は準備失敗になる |
| 受信からコピー送信へ | [forwarding](#forwarding) | ingressのreceivedを起点に経路照合・処理遅延・hop判定・コピー生成を進め、egressのTX処理・容量・仲裁へ引き渡す。各バスのCAN時間・ACK契約は基準CANから再利用する |
| 終了時の関係と互換 | [payload-results](#payload-results) | origin/parent/childで元Request・転送行・コピーRequestを結び、元成功とコピーの破棄・未完了を別に判定する。共通schema2と既定CAN schema1の回帰を合わせて確認する |

経路一致後の`submitted`はコピーRequestを生成した事実を表す。転送先queue_fullやEOF前の打切りでも元Busで確定した送信成功は保持される。反対に、GW処理期限がTなら転送行はprocessingのままで、コピーRequestはまだ存在しない。この二つを区別して、転送行の保存則とCAN要求の保存則をそれぞれ照合する。

親要件は構成の[DIR-AC-0028](../../要件定義書.md#dir-ac-0028)、転送の[DIR-AC-0029](../../要件定義書.md#dir-ac-0029)、結果・互換の[DIR-AC-0030](../../要件定義書.md#dir-ac-0030)を組み合わせて確認する。共通基盤のprofile登録・時刻・結果包絡は[拡張モデル共通仕様](../拡張モデル共通詳細機能仕様書.md)を参照する。

<a id="configuration"></a>

## 1. 構成・入力・準備検証

```trace
{
  "id": "spec-gw-models#configuration",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0124",
    "DIR-REQ-0125",
    "DIR-REQ-0129",
    "DIR-REQ-0130"
  ],
  "upstream": [
    "DIR-FUNC-0028"
  ],
  "state": "confirmed",
  "pending": []
}
```

担当機能は[独立バスとGW構成](../../機能仕様書.md#dir-func-0028)。

| 入力 | 確定契約 |
| --- | --- |
| INI | `[General] model-profile = "can.cc.multibus.v1"`、`model-config = "routing.json"`。相対パスはINI親基準。workloadは共通schema2（互換入力としてschema1も受理）で、CAN generatorのフィールド・kind・ordinalは[既存負荷](CANモデル詳細機能仕様書.md#workload)と同じ |
| 構造 | 1個以上のBus、各Busは2個以上のControllerと対のtx/rx接続を持つ。Controllerは一つのBusに属す。全Bus.profileは選択profileと一致。bitrateはバス別1..1000000整数bps。各バスで独立仲裁し、共通ps時刻で同時SOFを受理する |
| GW表現 | NEDのmoduleインスタンスにrouting coordinatorを付与する。登録済み`dir.can.MultibusController`と`dir.can.MultibusBus`はCANロジックを再利用し、固定multibus descriptorを持つ別実装キーとする。module配下にある異なるBusへ接続した2個以上の`dir.can.MultibusController`をportsに明記する。各Controllerは最大一つのGWに所属。GW所属portのgenerator指定は準備失敗（注）。通常Controllerだけがnative sourceとなる |
| NED登録schema | MultibusControllerは既存Controllerと同じqueueCapacity/txProcessingDelay/rxProcessingDelay/rxFilter、tx出力/rx入力を宣言する。MultibusBusはbitrate/profileとtx_SUFFIX入力/rx_SUFFIX出力の対を宣言する。FixedDelayは既存実装を共用。変化点は固定protocol/message schemaとprofile validatorだけで、値域・方向・対の規則は既存NED仕様と同じ |
| routingルート | 必須`{schema_version:1,gateways:Gateway[]}`。gateways=[]は独立バスのみの構成。未知・欠落・重複JSONキー、booleanを整数に見なす入力、型不一致を準備失敗とする |
| Gateway | 必須`node`=既存moduleパス、`ports`=一意Controllerパス配列、`routes`=Route配列。任意`processing_delay`=時間文字列、既定`0ps`、整数ps換算0..u64max。任意`hop_limit`=JSON整数1..65535、既定16。node重複は準備失敗。routes=[]を受理する |
| Route | 必須`id`=ASCII `[A-Za-z_][A-Za-z0-9_]*`でGW内一意、`ingress`=所属portパス、`egress`=空でない一意所属portパス配列、`format`=`standard`または`extended`、`id_min/id_max`=JSON整数。0≤min≤max≤2047/536870911。egressはingress以外、別Bus上のportとする |
| 経路重複 | 同GW・ingress・formatの包含ID区間は互いに素とする。端点だけ重なる場合も拒否。異なるformatは独立。一致する一行を区間照合で選ぶ。注：配列記載順の優先順位は対象外 |
| 所有者 | 各バスの(format,id)の送信者を一意にする。native generatorのID点と全経路のegress予約区間を検査する。到達不能・発火0件・フィルタで除外される区間も予約する。同じsourceへの重複予約は統合、異なるsourceの交差は準備失敗。これにより全動的コピーの所有を生成前に確認する |
| 循環 | formatごとに経路の全minとmax+1でID空間を分割する。各部分区間のBus有向グラフへingress Bus→egress Busを加え、有向閉路を検出した場合に準備失敗。format/IDが異なる辺を混ぜた見かけ上の閉路は受理する。診断に区間・経路ID・Bus列を含める |
| 診断・初期化 | 構成→JSON→範囲/重複→所有者→循環→native frame計算の順。準備不正は原因node/route/fieldをE-0001準備診断へ付与して実行開始を抑止。共通診断の位置・順序を適用する。記載順の変更は妥当な入力の結果へ影響させない |

<a id="forwarding"></a>

## 2. 転送・キュー・識別・終了

```trace
{
  "id": "spec-gw-models#forwarding",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0125",
    "DIR-REQ-0126",
    "DIR-REQ-0127",
    "DIR-REQ-0128",
    "DIR-REQ-0131"
  ],
  "upstream": [
    "DIR-FUNC-0029"
  ],
  "state": "confirmed",
  "pending": []
}
```

担当機能は[GW転送](../../機能仕様書.md#dir-func-0029)。CANのEOF成功・Receiver受信完了を転送入力に使う。

| 条件・段階 | 結果 |
| --- | --- |
| 受信前提 | 元BusのEOF後に、source tx経路+ingress rx経路の観測遅延を各一回加算。ingress rxFilterで適合したReceiverだけがrxProcessingDelay後receivedへ進む。フィルタ拒否は既存Receiver.filteredで終端し、GW転送行を作る対象外（注）。ACKはフィルタと独立 |
| 経路照合 | received時刻に(format,id,ingress)を照合。一致なしは転送行1個をstatus=filtered、reason=no_routeとして即時終端。元Request.success/Receiver.receivedを保持する |
| 複製 | 一致行のegressをUTF-8パス順に処理してコピーごとの転送行を作成する。format/id/dataを完全維持。処理タイマはコピー単位に持つ。入力Receiverは一回だけroutingへ渡す |
| 識別 | nativeはorigin_request_id=request_id、parent_request_id=null、gw_hops=0。コピーIDは`gw:<parent_request_id>/<gw_node>/<route_id>/<egress_path>`、originは不変、parentは直前送信ID、gw_hops=parent+1。元generator IDには`:`と`/`がないため一意。コピーIDは終端破棄を含む転送行で生成し、実際に送信生成された場合に同じIDをRequestへ使用する |
| GW処理 | `forward_due=ingress.received_ps+processing_delay`。received時に転送行status=processing、planned_forward_psを設定。正delayなら完了phase0→forward phase1、0なら同phase1に予約。計算不能は共通時間算術失敗。forward_due≥Tは処理待ち行を残す |
| hop | forward到達時に新gw_hops>当該GW.hop_limitならstatus=dropped、reason=dropped_hop_limit。Requestを作らず転送行で終端する。等値は受理。異なるGWの上限はそれぞれ到着コピーに対して評価する |
| 送信生成 | hop適合時にコピーRequestをprocessingとして生成し、generated_ps=forward_due。egressのtxProcessingDelayを加算しreadyへ。入出力処理をそれぞれ一度適用する。送信量・bitrate・仲裁は転送先Busに属す |
| 出力容量 | egress ControllerのqueueCapacity既定64、0..u32max、drop-newest。ready phase1の待機数と容量を比較。送信中・tx処理中は待機数から除外。容量0は全コピーをready時にqueue_full破棄。既存待機・他egressのキューは保持する |
| ローカル順序 | CAN仲裁列最小、同値ならegressへのready投入のコミット順。ready時刻が等しい場合はscheduler event sequenceで決まり、同一受信の複製はegressパス順で決定する。native Controllerのgenerator ID/ordinal規則は維持する |
| 要求独立性 | 元バスの成功を転送先のqueue_full、hop破棄、停止境界と独立した確定結果として保持する。別Busのbusy状態は別context。転送先競合中でも元Busはrelease後に次要求を開始する |
| 停止 | [0,T)のみ実行。GW処理中、TX処理中、pending、in_flight、成功後受信待ちを別々に保存。forward_due=TはコピーRequestなしのprocessing転送行。コピーEOF=Tはin_flight。終了後のキュー排出・成功補完は対象外（注） |

<a id="payload-results"></a>

## 3. 型付きpayload・結果・互換性

```trace
{
  "id": "spec-gw-models#payload-results",
  "stage": "spec",
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
    "DIR-FUNC-0030"
  ],
  "state": "confirmed",
  "pending": []
}
```

担当機能は[GW観測と互換性](../../機能仕様書.md#dir-func-0030)。結果は[共通schema2](../拡張モデル共通詳細機能仕様書.md#results)を適用する。

| payload | 正確な契約 |
| --- | --- |
| 登録 | `can.cc.multibus.v1.CanTxRequest`と`can.cc.multibus.v1.CanNotification`、version=1。ポートprotocolは`can.cc.multibus.v1`。MultibusController/MultibusBusの固定descriptorがこの型を付与する。idealの実装キー・codecを保持する |
| codec | compact UTF-8 JSON、下記記載順でキーを出力。未知・重複・欠落キーを拒否。Dは非負正規十進文字列、時刻u64、hopsは0..65536（上限超過コピーを表現）、Pは正規パス、Iは要求ID、Fは既存CanFrame。各キー必須、nullableを?で示す |
| Tx | `{schema_version:1,profile:"can.cc.multibus.v1",request_id:I,source_id:P,bus_id:P,frame:F,generated_ps:D,ready_ps:D,origin_request_id:I,parent_request_id:I?,gw_hops:D}`。Envelope宛先Bus・source経路・ready時刻と登録Requestを照合。tx経路delayはreadyへ加算する対象外（注） |
| Rx | `{schema_version:1,profile:"can.cc.multibus.v1",request_id:I,source_id:P,receiver_id:P,bus_id:P,frame:F,generated_ps:D,sof_ps:D,eof_ps:D,observed_ps:D,origin_request_id:I,parent_request_id:I?,gw_hops:D}`。Envelope宛先/時刻、登録Receiver、frame、origin/hopsを照合 |
| frame保持 | native frameはprepare時に全値のWireFrameを計算。コピーは元要求の不変WireFrameを共有し、generated時からCRC/stuff/frame_bits/serialized_bitsを全て持つ。bitrateと予定EOF/releaseだけは転送先で設定。未観測payloadを推測して生成する処理は対象外（注） |

| 出力 | schema2での契約 |
| --- | --- |
| Request | model_recordsのschema_name=`can.request`、schema_version=1、record_id=request_id、subject=source、request_id=request_id、origin_request_id=origin。dataは共通Request全キーとCAN内訳を保持する。time_psは生成・ready・SOF・EOF・releaseの最後の到達時刻。model_fields.profileはmultibus、schema_version=1。CAN内訳にorigin_request_id、parent_request_id、gw_hopsを追加したMultibusFieldsを使用。時刻/状態/attemptの意味は既存CANを再利用する |
| Receiver | schema_name=`can.receiver`、schema_version=1、record_id=`<request_id>/<receiver_path>`、subject=receiver、request_id=親Request ID、origin_request_id=origin。dataは既存Receiver全キー。time_psは作成時EOF、観測時observed、完了時receivedの最新到達時刻。schema2ではRequest/Receiverもmodel_recordsへ格納し、simulationに並列requests/receivers配列は設けない（注） |
| 転送行 | simulation.model_recordsの`gw.forward`版1。下記の全fieldを必須とする。転送行はingress received時だけ生成。共通envelopeのschema_nameは`gw.forward`、schema_version=1、record_id=forward_id、subject=gateway、request_id=parent_request_id、origin_request_id=origin。time_psは最後にcommitされた行の変更時刻（作成時received、転送到達時forwarded）。dataに下記内訳を格納し、共通規則で(schema_name,subject,record_id)順に出力する |
| 内訳 | `{forward_id:I,parent_request_id:I,origin_request_id:I,gateway:P,ingress:P,egress:P?,route_id:S?,gw_hops:D,received_ps:D,planned_forward_ps:D?,forwarded_ps:D?,child_request_id:I?,status:S,reason:S?}`。一致経路はforward_id=コピーID、egress/route/予定が非null。no_routeは`filtered:<parent>/<gateway>/<ingress>`、egress/route/予定/forwarded/childはnull、hops=親の値 |
| 状態 | processingはforwarded/child/reason=null。forward到達でhop破棄ならstatus=dropped、forwarded=到達時刻、child=null、reason=dropped_hop_limit。生成成功ならstatus=submitted、forwarded=生成時刻、child=forward_id、reason=null。以後のpending/drop/successは参照Requestで判定。no_routeはstatus=filtered/reason=no_route。submittedを成功の同義にする処理は対象外（注） |
| Record計測 | 既存CAN指標はBus/Controller別。追加metric version1は`gw_copy_created`、`gw_copy_submitted`、`gw_hop_dropped`、`gw_route_filtered`、`gw_processing_pending`。unit=count/value_kind=integer、target=GWパス、request_id=親ID、receiver=egress（no_routeはingress）、reasonは対応理由、増分1の点標本。processing_pendingだけ終了時点の全体集計でrequest_id/receiver/reason=null、valueは残processing数。共通Recordの残フィールドはnull規則を適用 |
| 完全行例 | [model-record-examples.json](../../verification/fixtures/gw/model-record-examples.json)はindependent入力のコピーRequest・Receiver・転送行の全fieldを持つ解析例。3行のprojectionであり、実行結果全体又は実測証跡ではない（注） |
| schema登録 | metadata.model_schemasへ`can.request`/1、`can.receiver`/1、`gw.forward`/1を登録。ModelRecord.dataは各完全field集合で検証し未知キーを拒否する。metadata.model_profileはmultibus、model-config原文・正規化値・hashとBus別bitrate・GW設定を保存する |
| 指標descriptor | 追加5指標はversion=`1`。先頭4個はsampling=`point`、aggregation=`identity`、到達時刻time_psとevent_seq/effect_seqを持つ。processing_pendingはsampling=`summary`、aggregation=`sum`、start_ps=0/end_ps=H、time_ps/event_seq/effect_seq=null。各GWに0件でもpending集計1行を出す。sample_countは全てnull。元CANの全37指標を併せて登録する |
| コピー数の母数 | createdは一致経路のegress数、submittedは実Request生成数、hop_droppedは生成前破棄数。H時点でcreated=submitted+hop_dropped+processing_pending。filteredは母数外。queue_fullはsubmitted Requestのdropped内数 |
| origin集計 | originごとにRequestをグループ化しnative1件・転送コピー件数・成功/破棄/未完了を別に数える。末端はGWのportsに属さないControllerのReceiver.receivedと定義し、その時刻−origin Request.generated_psを経路遅延とする。no_route/hop破棄は配達成功の標本へ加えない（注）。分岐の平均を元要求数へ混ぜず、末端receiverごとに値を保持する。元IDからRequestと転送行の親子をたどれる |
| 互換検証 | ideal profileの入力schema1・出力schema1・全bitvector/既存時刻projectionを回帰比較する。multibusを選ぶことで同じ1バス構成も受理するが結果schema2を使う |

詳細処理は[GW設計](../../design/GWモデル詳細設計書.md)、具体例は[GW検証](../../verification/cases/GWモデル検証仕様書.md)を正とする。
