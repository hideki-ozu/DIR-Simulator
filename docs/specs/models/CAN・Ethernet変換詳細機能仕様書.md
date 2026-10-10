# CAN・Ethernet変換詳細機能仕様書

文書バージョン：`1.1.0`
設計日：`2026-10-07`
対象GitHubバージョン：`main @ 2f1e60b`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）
文書ID：`spec-can-ethernet`
文書状態：未公開。抽象モデルを実装済みで、native CLI・schema2出力・Viewerに対応する。実施した製品試験と未照合の組合せは検証仕様の実施記録に区別して示す。規格全体適合の証明ではない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | 初回pushに向け、CAN↔Ethernet変換の入力・codec・方向別規則・容量・時間・lineage・結果契約と実装範囲を確定 |

[詳細設計](../../design/CAN・Ethernet変換詳細設計書.md)と[検証仕様](../../verification/cases/CAN・Ethernet変換検証仕様書.md)を対とする。ACは[DIR-AC-0059、0060](../../要件定義書.md)に対応する。

<a id="codec"></a>

## 1. 複合profile・入力・codec

```trace
{"id":"spec-can-ethernet#codec","stage":"spec","requirements":["DIR-REQ-0230","DIR-REQ-0231","DIR-REQ-0232","DIR-REQ-0233"],"upstream":["DIR-FUNC-0058"],"state":"confirmed","pending":[]}
```

### 1.1. 選択と有効構成

`can.ethernet.gateway.v1`を明示選択する。一つの整数ps FESにClassical CAN標準11bit・拡張29bitデータフレーム、全二重Ethernet、静的VLAN/QoSとGatewayを置く。CAN FD、RTR、半二重、媒体固有PHY、TSN、動的制御は準備拒否する。既存13 profilesの選択・結果契約は変更しない。

各CAN Controllerは一つのBusに所属。Ethernet接続成分はconnected tree、GatewayはCAN Controller一個以上とEthernet Endpoint一個を所有するcompoundであり、Controller/Endpointの重複所有を拒否する。Gatewayを跨ぐ論理経路は循環を許すが、1.4のlineageで停止する。Ethernetの物理ループは許さない。CAN→CAN、Ethernet→Ethernetの直接変換規則はこのprofileでは拒否する。Switchによる通常L2転送は許す。

### 1.2. 入力契約と具体例

INIは既存共通キーを使用する。実行可能な [R01 CAN→Ethernet](../../../examples/can-ethernet/r01.ini)、[R02 Ethernet→CAN](../../../examples/can-ethernet/r02.ini)、[双方向](../../../examples/can-ethernet/bidirectional.ini)、[二Bus fanout](../../../examples/can-ethernet/fanout.ini)、[有限RX](../../../examples/can-ethernet/rx-limit.ini) を同梱する。以下は共通INIキーの例であり、実際のfile名・network名は各INIを参照する。

```ini
[General]
network = bridge.Main
ned-path = "models"
model-profile = "can.ethernet.gateway.v1"
model-config = "model.json"
workload = "workload.json"
sim-time-limit = 1ms
```

NED型の以下の宣言を登録する。省略のない最小接続骨格を示す。`CanBus`のbitrateは500kbps、ControllerのqueueCapacityは64、tx/rxProcessingDelayは0ps、Ethernet Endpointはtx/rx一対、`Link`は1Gbps/0psを既定とする。以下の型はこの既定値を持つ新profile専用登録とする。

```ned
package bridge;
simple CanController { parameters: @class("dir.bridge.CanController"); gates: input rx; output tx; }
simple CanBus { parameters: @class("dir.bridge.CanBus"); gates: input rx_a; input rx_b; output tx_a; output tx_b; }
simple EthEndpoint { parameters: @class("dir.bridge.EthEndpoint"); gates: input rx; output tx; }
channel Link { parameters: @class("dir.bridge.EthLink"); }
module Gateway {
 gates: input can_rx; output can_tx; input eth_rx; output eth_tx;
 submodules: can: bridge.CanController; eth: bridge.EthEndpoint;
 connections:
  can_rx --> can.rx; can.tx --> can_tx;
  eth_rx --> eth.rx; eth.tx --> eth_tx;
}
network Main {
 submodules: src: bridge.CanController; bus: bridge.CanBus; gw: bridge.Gateway; sink: bridge.EthEndpoint;
 connections:
  src.tx --> bus.rx_a; bus.tx_a --> src.rx;
  gw.can_tx --> bus.rx_b; bus.tx_b --> gw.can_rx;
  gw.eth_tx --> bridge.Link --> sink.rx;
  sink.tx --> bridge.Link --> gw.eth_rx;
}
```

submoduleの型参照はpackageを含む完全名を使う。処理遅延・queue容量等の例別変更はINI overrideで指定する（例：`R01.gw.can.rxProcessingDelay = 1us`）。submodule内のinline parametersには依存しない。

本profile model-config rootは必須の `{schema_version:1,can:{},ethernet:{},gateways:[]}`。canは既存multibusの静的Controller/Bus解決値（NED由来）を使い、この版では空object以外を拒否する。ethernetはVLAN model-config schema_version=3の完全objectで、GatewayのethもEndpointとして含む。既存[Ethernet VLAN入力](EthernetVLAN・マルチキャスト詳細機能仕様書.md#ports-codec)のendpoints/switches/ports/outputsは省略不可。以下はrootのgateways配列の具体的な一要素である。

```json
{
  "instance":"Main.gw", "can_ports":["Main.gw.can"], "ethernet_endpoint":"Main.gw.eth",
  "rx_capacity":2, "conversion_delay_ps":"2000000", "max_hops":8,
  "rules":[
    {"id":"to_eth","direction":"can_to_ethernet","ingress":"Main.gw.can",
     "match":{"format":"standard","can_id":0}, "egresses":[
       {"port":"Main.gw.eth.tx","dst_mac":"02:00:00:00:00:02","vid":10,"pcp":3}]},
    {"id":"to_can","direction":"ethernet_to_can","ingress":"Main.gw.eth",
     "match":{"vid":10,"pcp":3,"format":"standard","can_id":0}, "egresses":[
       {"port":"Main.gw.can.tx","format":"standard","can_id":0}]}
  ]
}
```

全field必須。整数fieldはJSON整数、boolを拒否。時刻はu64正規十進文字列。rx_capacity=0..u32::MAX、max_hops=1..255、formatは`standard`又は`extended`、can_idはstandardで0..2047、extendedで0..536870911 (0x1fffffff)、vid=1..4094、pcp=0..7。rule IDはGateway内で一意なASCII `[A-Za-z0-9_-]+`。rule照合はexact tupleのみとし、同方向・同ingressで一致範囲が重なるruleを拒否する。優先順位や先勝ちはない。egressesは空不可、一規則で同一port重複不可、所有する反対媒体のportのみ。CAN→EthernetはEthernet Endpointが一個のため一枝、Ethernet→CANは複数CAN出力へfanout可能。複数Gatewayへのfanoutはbroadcast又は静的multicastを使用する。CAN ID→PCPはto_ethの固定値、PCP→CAN IDはto_canの固定値で定義し、大小反転を暗黙に推測しない。異なるCAN IDが同PCPへ写ることは許す。両方向のmatchはformatとcan_idを必須とし、Ethernet→CANのegressにも出力formatとcan_idを必須とする。CAN→Ethernetは元format/IDをcodecへ保存する。Ethernet→CANは入力codecのformat/IDで照合し、明示された出力format/IDへ書き換える。standardのID291とextendedのID291は別keyであり、format非一致はno_rule。入力形式の範囲超過はinvalid_codec、設定内の範囲超過はprepare拒否。

workload rootは `{schema_version:1,can:CAN_WORKLOAD,ethernet:VLAN_WORKLOAD}`。内部objectは既存それぞれのworkload schemaをそのまま使用し、CAN workloadはGateway所有Controllerを送信元にできない。Ethernet workloadはGateway所有Endpointを送信元にできない。VLAN workloadのframe.dataには次節のcodec hexを指定できる。外部Ethernet起点は到着時にoriginを一度だけ割り当て、利用者から内部lineageを受け取らない。具体的なworkload.jsonは次のとおり。一件のCAN起点と100usのEthernet起点を含む。

```json
{
  "schema_version":1,
  "can":{"schema_version":2,"generators":[
    {"id":"can_source","kind":"can.periodic.v1","node":"Main.src",
     "start":"0ps","period":"1ms","end":"1ms",
     "frame":{"format":"standard","id":0,"data":""}}
  ]},
  "ethernet":{"schema_version":3,"generators":[
    {"id":"eth_source","node":"Main.sink","kind":"ethernet.explicit.v1",
     "times_ps":["100000000"],"frame":{"dst_mac":"02:00:00:00:00:01",
       "ether_type":34997,"data":"4449524301000000000000","tag":{"vid":10,"pcp":3,"dei":0}},
     "flow_id":"reverse","priority":3,"deadline_ps":null}
  ]}
}
```

この例のEthernet設定はMain.gw.ethのMACを02:00:00:00:00:01、Main.sinkを02:00:00:00:00:02とする。両portのPVID=10/admit=all/default_priority=3/vlans=[{vid:10,tagged:true}]、switches=[]、各outputをstrict priorityの8class（各64frame、byte上限なし）として完全列挙する。未知/重複キー、未知instance、将来時刻やcount=0の不正設定もprepare時に拒否する。

### 1.3. DIR実験wire codec v1

inner EtherType=34997 (`0x88B5`)を固定する。これは[IANA IEEE 802番号表](https://www.iana.org/assignments/ieee-802-numbers/ieee-802-numbers.xhtml)および[RFC 9542](https://www.rfc-editor.org/rfc/rfc9542.html)のlocal experimental割当であり、DIR専用の標準割当ではない。本codecはDIR独自で、AVTP、SOME/IP、IEEE 1722等への準拠は主張しない。

| offset | bytes | 内容 |
| --- | --- | --- |
| 0 | 4 | magic `44 49 52 43` (DIRC) |
| 4 | 1 | version=1 |
| 5 | 1 | flags bit0=extended（0=standard、1=extended）。bit1..7は予約0 |
| 6 | 4 | CAN ID、big-endian。standardは上位21bit=0、extendedは上位3bit=0 |
| 10 | 1 | DLC=0..8 |
| 11 | DLC | CAN data |

codec長は11+DLC=11..19byte。standardのID0/DLC0は `4449524301000000000000`、standardのID291/DLC2/data aabbは `4449524301000000012302aabb`。extendedのID536870911/DLC8/data全FFは `4449524301011fffffff08ffffffffffffffff`。CRC-15、stuff bits、CAN EOF/intermissionを格納しない。Ethernet paddingはcodecの外側のzeroで、dataを46byteまで埋める既存serializerを使用。受信decodeはDLCから有効長を求め、残余がすべてzeroか検査する。長さ不足、DLC超過、flags/version/magic不正、ID上位bit、非zero残余、FCS不正は拒否。VLAN有りは既存の68byte、無しは64byte最小MAC frameとなる。tagged→untaggedでcodec/paddingを変更せずFCSのみ再計算する。CAN送出時は出力ruleのformat/ID、復号dataからCAN CRC-15とstuffingを再生成する。

### 1.4. 入場・変換判定

CANの通常rxFilter、EthernetのFCS→admit→VID membership→MAC宛先の順に判定後、codec→lineage→rule照合→RX容量の順。GatewayのEthernet宛先は自身のindividual MAC、broadcast、又は当該VIDで購読するgroupに限る。他individual宛てをpromiscuousに変換しない。該当ruleなしは`no_rule`、codec不正は`invalid_codec`、既訪問Gatewayは`loop_prevented`、次の変換でmax_hopsを超える場合は`hop_limit`、RX満杯は`rx_full`。通常受信拒否と変換拒否を別の理由列へ保存する。

内部lineageはwireに含めない。origin_idはnative元要求/frameの固定ID、parent_idは直前の転送record ID、conversion_idは`origin_id/Gateway/ingress_record/rule_id`を構成要素の長さ付き符号化で一意に作る。child IDはconversion_idと辞書順egress indexから生成する。Gateway訪問列は各枝が独立所有し、変換受理時に自身を一回appendする。同一originの別枝の正当な到着をglobal seen setで捨てない。複合run内部ではcoordinatorのtyped frame lineageによりSwitchを跨いで保存する。外部再投入を同一originと推定しない。

<a id="runtime"></a>

## 2. 時刻・容量・結果

```trace
{"id":"spec-can-ethernet#runtime","stage":"spec","requirements":["DIR-REQ-0230","DIR-REQ-0234","DIR-REQ-0235","DIR-REQ-0236","DIR-REQ-0237","DIR-REQ-0238","DIR-REQ-0239"],"upstream":["DIR-FUNC-0059"],"state":"confirmed","pending":[]}
```

### 2.1. store-and-forwardと保持

CAN ingress observed=EOF+受信伝搬遅延、Ethernet ingress observed=EOF+link delay。受信が入場したobservedでGateway共通RX slotを一つ予約し、ready=observed+ingress rxProcessingDelay+conversion_delay。CAN受信伝搬は既存経路定義を継承し重複加算しない。ready以後、egress固有txProcessingDelayを一回だけ足してofferする。CAN/Ethernetの送信器はSOF以後にのみ媒体占有を開始し、Gateway処理時間を媒体busyへ含めない。

RXはprocessing→waiting_tx→released。全枝のTX受理又はterminal dropが確定した時だけreleaseする。TX容量不足はdropせず待機し、CANはSOF、Ethernetはqueue dequeue/SOFで容量を解放して同時刻に再試行する。容量0又は一枝のMAC bytesがbyte上限より大きい場合は永久待機を避け`tx_unadmittable`のterminal drop。CAN queueは待機request件数、Ethernetは既存8classの待機件数/MAC bytesで数え、送信中は含めない。全枝preflightはID/算術/予約/参照エラーによる部分commitを防ぐ。容量可否は枝別の正常結果で、受理済み枝を再送せず未受理枝だけ待つ。待機者順は(ready,origin_id,conversion_id,egress_path)で固定する。

単一FES keyは(time_ps,delta,phase,seq)。phase0はcompletion/control、phase1はarrival/offer、phase2は仲裁。遅延0の受信処理・変換はphase1 callback内でofferまで計画し、同deltaの仲裁に参加する。正遅延はphase0 timerからphase1 offerを予約。phase2で解放された容量から起こるofferは次deltaで処理し、過去phaseへ戻らない。CAN同時候補は既存CAN ID仲裁、同ID競合は既存拒否規則。Ethernetは既存schedulerとPCP class FIFOを使う。

### 2.2. 記録と受信母数

native CAN/Ethernetのgenerator名は別名前空間であり、同名を許す。既存媒体recordのIDは変更せず、追加bridge recordのroot `origin_id`を `can:<native request ID>` / `ethernet:<native frame ID>` とする。native元segmentの `segment_id` はこの修飾originと同じ値、`source_record_id` は既存媒体のIDとする。変換childは修飾origin・branchを含む長さ付きIDから派生する。参照はschemaとIDの組で解決し、native元segmentはorigin媒体、変換segmentは最後のrule方向で媒体を選ぶ。completion参照もsegment媒体に従う。旧未公開結果の裸originはnative候補が一つの場合だけ解決し、曖昧なら拒否する。媒体優先の検索は行わない。


schema2 `simulation.model_records`を正本とし、共通summaryをCAN/Ethernetの異種件数の単純和でsuccess率へ変換しない。追加schemaは`dir.can_ethernet.conversion/1`、`dir.can_ethernet.branch/1`、`dir.can_ethernet.segment/1`。conversionはorigin_id/parent_id/gateway/ingress_record/rule_id/visited_gateways、observed/ready/releasedの実績ps（未発火null）、planned_ready、RX状態・理由を持つ。branchはconversion_id/egress/child_id/codec_length/pcp/映射前後CAN format/ID、offer/admitted/sofの実績ps、waiting/admitted/dropと理由を持つ。CAN側は `can.request/1`・`can.receiver/1`、Ethernet側は `ethernet.frame/3`・`ethernet.transfer/3`・`ethernet.reception/2` の既存recordの意味を再利用する。native CAN requestの `model_fields.profile` は `can.cc.multibus.v1`、`origin_request_id` は当該request自身、`parent_request_id` はnull、`gw_hops` は0とし、媒体間lineageは新recordだけに保存する。複合profileの登録schema集合をmetadataへ保存する。

受信母数は二種類を混ぜない。媒体受信率は各実SOF時に対象集合を固定する。CANは送信者以外の接続ControllerでrxFilterを満たす集合、Ethernetはsource VIDで宛先適合するEndpoint集合（individual=当該1台、broadcast=source以外のVID member、group=購読member、Gatewayを含む）。対象0の率はnull。同一originが分岐して同Endpointへ複数回届く場合、媒体のsource transmission単位に集計する。受信完了実績のない対象はpartialでも成功へ数えない。

変換母数はGatewayに実際に到着して通常媒体受信を通過したingress件数であり、未来の経路から生成しない。`attempted = accepted + rejected`、`accepted = processing + waiting_tx + released`、`branches = waiting + admitted + dropped`を満たす。EthernetのSwitch hop受信をGateway変換試行へ数えない。end-to-endの分母は、実際にSOFを開始した媒体segmentごとに凍結したterminal対象tupleの総数とする。terminalはGatewayに所有されない通常application Controller/Endpointであり、中間Gatewayの受信は媒体受信母数に含めるがend-to-endには含めない。tupleは `(origin_id,segment_id,branch_lineage,terminal_id)`。CANは当該bus送信、Ethernetはnative/変換生成frameの最初の送信を一segmentとし、Switchによるhop転送は同一segmentを継承して新しい分母を作らない。native元送信も明示的segment_idを持ち、branch_lineageは空列とする。変換childはchild ID由来の別segment_idとその枝lineageを持つ。同じterminalへの異なるsegment/lineageは独立機会であり、同じtupleの複数受信は一回だけ完了と数える。

segment SOF時の媒体対象集合からGatewayを除いた対象集合を凍結し、その集合の大きさだけ分母を増やす。未来の変換先や未SOF枝から対象を合成しない。受信処理まで実完了したtupleのみ分子へ一度加算する。`0 ≤ completed ≤ target_count`、率はcompleted/target_count、分母0はnull。各tupleの遅延はorigin生成からそのterminal受信処理完了までとし、未完成tupleは平均に含めない。

`dir.can_ethernet.segment/1`はsegment_id/origin_id/source_record_id/branch_lineage、実sof_ps、sorted target tuples、target_count、各tupleのcompleted_ps（未完了null）とcompletion_reception_id（未完了null）を保持する。SOFと対象集合は同じ成功batchで記録し、その後は完了列だけを更新する。native元segmentも同じschemaを使用するため、結果だけから独立に分母と実績を再構築できる。

metric名は`gateway.conversion.attempted/accepted/rejected`、`gateway.rx.occupancy/max`、`gateway.branch.waiting/admitted/dropped`、`gateway.end_to_end.target_count/completed/latency_ps`。単位と対象所有者をRegistryへ登録する。MAC bytes、payload bytes、CAN bits、媒体別busyと遅延は別descriptorとする。

ViewerはCAN bus・Ethernet linkとGateway内部conversionを区別して表示し、選択originから媒体を跨いでparentを追う。plannedは実績の代用にしない。未到着・processing・waiting_tx・dropを表示、前進/巻戻しは同じ到達区間、連続再生は線のみ強調する。quiet topologyと省略数を保持する。

### 2.3. 診断・停止

準備失敗はINI/JSON pointer/NED path/field/理由を持ち、部分runは開始しない。runtime診断は`profile,time_ps,event_seq,gateway,rule_id,branch_id,reason`を可能な範囲で含める。算術overflow、event ID枯渇、予約失敗、codec内部不整合はcallback全体を未commitとして停止。正常なcodec拒否や満杯待機はrun失敗にしない。

[0,T)を使用し、T丁度のEOF/到着/ready/SOFは未発火。実績列はnullを維持し、予定列とpendingを保存する。max_events/delta上限/キャンセル/公開失敗は共通終了契約を継承し、失敗callback以前の確定journalだけを公開する。入力配列順に依存しないcanonical path/rule sortでID、seqと結果を決定し、旧profileは既存fixturesで回帰確認する。
