# Ethernetモデル詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 45ce163`
予定公開版：`v1.1.0`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-04` | 全二重L2の開発実装・製品fixture検証を記録し、媒体v2との提供境界を明示 |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：Ethernet L2の入力・時間・蓄積交換・結果契約を確定。親要件の交換責務を追跡し、共通profile・codec登録境界と照合。CSMA/CD半二重・1000BASE-T1の媒体別profileと互換境界を追加 |

文書ID：`spec-ethernet-models`

| 項目 | 内容 |
| --- | --- |
| 適用 | 本書の基準契約はethernet.l2.store-forward.v1。半二重と1000BASE-T1はv2の[媒体仕様](Ethernet媒体拡張詳細機能仕様書.md)を併用 |
| 状態 | 全二重L2の開発実装・製品試験を実施。媒体拡張v2は未実装。実施範囲は[検証記録](../../verification/results/ethernet-v1-2026-10-04.json)で管理 |

## L2通信を確認する三つの母数

[Ethernet L2通信](../../要件定義書.md#dir-req-0145)の親要件は、元frameの生成、方向別linkでの送信、Switchでの交換、endpointでの受信をつないで判断する。全二重L2は開発中のソースへ実装した。公開タグの提供状態、媒体拡張、規格全体への適合は別に管理する。

| 確認段階 | 本書の担当節 | 評価の単位 |
| --- | --- | --- |
| 入力とframe構築 | [profile](#profile)・[wire](#wire) | 準備時にMAC bytes・padding・FCSを確定し、generatorの発火ごとに元frameを一回生成する。接続tree、MAC、FDB、方向対の不整合は実行開始前に診断する |
| 送信と蓄積交換 | [wire](#wire)・[switch](#switch) | hopごとのtransferをoutput FIFOへofferし、SOF/EOF/release/arrivalを記録する。Switch到達後の処理完了でFDB又はfloodの出力copyを決める |
| 到達と終了結果 | [outputs](#outputs) | arrivalごとのreceptionから受信・forward・filter・処理待ちを判定し、frame→transfer→receptionと子transferを関連付ける。共通停止・journal・窓集計を適用する |

一つのbroadcast frameが複数egressへ分岐すれば、generatedは一件でもtransferと受信件数は増える。満杯egressだけのcopy破棄、別egressの成功、EOF済みでarrival未到達のcopyを同じ実行に保持できることを確認する。serialized、release済み、endpoint receivedはそれぞれ異なる到達境界であり、送信量と受信遅延の標本を対応する境界から求める。

親要件の確認にはframe/linkの[DIR-AC-0034](../../要件定義書.md#dir-ac-0034)、交換の[DIR-AC-0035](../../要件定義書.md#dir-ac-0035)、結果・停止の[DIR-AC-0036](../../要件定義書.md#dir-ac-0036)を組み合わせる。半二重と1000BASE-T1は[媒体拡張仕様](Ethernet媒体拡張詳細機能仕様書.md)の差分を加え、選択したprofileごとの契約で判断する。

<a id="profile"></a>

## 1. プロファイル・構造・負荷

```trace
{
  "id": "spec-ethernet-models#profile",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0146",
    "DIR-REQ-0154"
  ],
  "upstream": [
    "DIR-FUNC-0034",
    "DIR-FUNC-0036"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| profile | INI `[General] model-profile = "ethernet.l2.store-forward.v1"`。EthernetII untagged、全二重、static FDB、固定処理遅延、無誤りlinkを組み合わせる。注：規格全体適合認証、PHY符号化、half-duplex/衝突、PAUSE、VLAN、QoS、STP、MAC学習/aging、IP/ARP、故障注入は本profileの対象外 |
| 登録型 | `dir.ethernet.Endpoint`: output tx/input rx、int queueCapacity既定64、double txProcessingDelay/rxProcessingDelay @unit(s)既定0ps。`dir.ethernet.Switch`: 2個以上の同名suffixを持つinput rx_suffix/output tx_suffix、int queueCapacity既定64。`dir.ethernet.Link`: directed channel、double bitrate @unit(bps)必須、double delay @unit(s)既定0ps。時間・容量はchecked整数へ解決 |
| 型と時間の所有 | Endpoint/Switchが各出力portのFIFO・MAC送信器を持つ。Linkは解決済bitrate/伝搬のdescriptorを渡し、送信器が直列化を一回だけ計算する。標準sendによるchannel遅延とモデルの到達予約を重ねて適用する処理は対象外（注）。モデル内部到達通知は指定の到達時刻で宛先modelへ届ける |
| 登録capability | 全Ethernet portはprotocol=`ethernet.l2`、message=`frame`、payload schema=`dir.ethernet.Arrival.v1`（version1）で一致させる。Linkはcapability=`ethernet-mac-link` version1としてbitrate_bps/delay_psを提供する。FixedDelayだけの経路は本profileの必要capabilityを満たす経路と区別する |
| トポロジー | 全Endpointは同一の一つのpeerへtx/rx一組、Switchは同一suffixのtx/rxを同一peerと結合する。各方向のLinkは同bitrate/同delayの対を持つ。平行接続・自己接続を拒否し、方向対を一辺とした無向グラフがtreeとなるconnected構造を受理する。Endpointは2個以上、Switchは0個以上。静的構造を実行全体で保持する |
| model-config | INI `model-config`相対パスのUTF-8 JSON。全キー必須の`{schema_version:1,endpoints:EndpointConfig[],switches:SwitchConfig[]}`。EndpointConfig=`{instance:S,mac:S}`、SwitchConfig=`{instance:S,forward_delay_ps:D,fdb:Fdb[]}`、Fdb=`{dst_mac:S,egress:S}`。各選択instanceを過不足なく一度列挙、egressは所有Switchの接続済output完全パス。forward_delayは0以上u64 ps |
| MAC | colon区切り6byte（例02:00:00:00:00:01）、入力大文字も受理して小文字へ正規化。sourceはnonzeroで先頭byte bit0=0、endpoint間で一意。destinationはnonzero unicast又はff:ff:ff:ff:ff:ff。未知unicastは受理する。FDB keyはunicastだけ、重複を準備失敗。注：broadcast以外のgroup宛先は対象外 |
| workload | INI `workload`のschema2=`{schema_version:2,generators:[{id:S,node:S,kind:"ethernet.explicit.v1",times_ps:D[],frame:{dst_mac:S,ether_type:integer,data:S}}]}`。idは実行内一意のASCII識別子、nodeはEndpoint完全パス。時刻列はu64 psの非減少（同値・空列可）、ordinalは入力位置0始まり。ether_typeは1536～65535、VLAN/QinQ用0x8100/0x88a8を除く。dataは0～3000偶数桁hex、空可、空白/0xなし。注：指定上位protocolはopaque bytesとして保持し解釈しない |
| 生成 | 時刻g<Tの要素だけ生成。全generatorの同時発火はid UTF-8辞書順、同idはordinal順。frame_id=`id:ordinal`。source MACはnode設定を使う。g+txProcessingDelayで最初のegressへoffer。正delay完了はphase0→phase1、0delayは同phase1通知。元frameを一回journalへ登録する |
| 準備検証 | 全JSON階層の未知/重複/欠落キー、boolによるinteger代用、負値/非正規D、時刻降順、未知node、未対応kind、MAC/port/FDB/トポロジー不整合をE-0001で準備失敗。入力位置とreasonは[診断仕様](../診断詳細機能仕様書.md#diagnostic-schema)。T以降・空generatorも全fieldを検証しframe bytesをcacheする |
| 資源値 | queueCapacity=0～4294967295、bitrateは10000000/100000000/1000000000/10000000000 bpsのいずれか、delayはu64 ps。モデル版が選ぶMAC平均レートであり特定PHYの実ビット列時間保証とは区別する |
| 一次資料と採用版 | [IEEE 802.3-2022](https://doi.org/10.1109/IEEESTD.2022.9844436)の規格識別と[IEEE 802.1Q-2022公開概要](https://standards.ieee.org/ieee/802.1Q/10323/)のbridge責務を参照。公開[IEEE作業部会資料の基本frame図](https://grouper.ieee.org/groups/802/3/cg/public/adhoc/cordaro_8023cg_short_reach_new_preamble_proposal_1220.pdf)および[IPG解釈資料](https://grouper.ieee.org/groups/802/3/maint/email/msg00142.html)を2026-09-29に確認。8byte preamble/SFDと12byte IFG固定は本抽象profileの明示選択。注：802.3/802.1Q全文の網羅照合、資料の提案方式の採用、全速度のPHY例外再現は実施範囲外 |

<a id="wire"></a>

## 2. フレームと独立方向の送信時間

```trace
{
  "id": "spec-ethernet-models#wire",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0146",
    "DIR-REQ-0147",
    "DIR-REQ-0148",
    "DIR-REQ-0149"
  ],
  "upstream": [
    "DIR-FUNC-0034"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| MAC bytes | DA6byte、SA6byte、EtherType2byte（network byte order）、data、pad、FCS4byteの順。data長L、pad=max(0,46−L)個の00、M=14+max(46,L)+4は64～1518。payload量は8L（pad除外）、mac_bits=8M、wire_bits=8(M+8)、occupied_bits=8(M+8+12) |
| FCS | DAからpad末尾までにCRC32を計算。c=0xffffffff、各byte bでc XOR b、その後8回、LSB1なら(c>>1) XOR 0xedb88320、LSB0ならc>>1。終端でc XOR 0xffffffffを32bitへマスク。FCS byteは結果の下位byteから4個、fcs_hexはそのwire byte順8桁小文字。bytes内はLSB先行。preambleは55を7個とd5を1個でCRC入力から分離。全hopで同じMAC bytes/FCSを保持 |
| FCSの適用範囲 | 出力は計算済valid FCSを持つ無誤り転送。利用者のFCS上書き・不正FCS注入は準備失敗。受信側が実装不変条件として再検算して不一致ならE-0002/model_failedで全停止（注：回線誤りの確率モデルではない） |
| 時間原点 | sof_ps=sはpreamble先頭。eof=s+ceil(wire_bits×10^12/R)、release=s+ceil(occupied_bits×10^12/R)、arrival=eof+link.delay。整数分子を先にchecked計算し、SOF原点で各ceilを独立適用。解放後だけ次SOF可。EOFとIFGのceilを別々に足し合わせる計算は対象外（注） |
| 同時進行 | tx→peerとpeer→txは異なる資源。各方向はidle/transmitting/interframeを持ち、EOFでinterframe、releaseでidle。到達は伝搬分だけずれ、受信処理と逆向き転送と次frame送信を重ねられる |
| Endpoint受信 | arrivalで受信行を作り、宛先が自MAC又はbroadcastならarrival+rxProcessingDelayでreceived。他unicastはarrival時にfiltered(reason=destination_mismatch)。受信dataは生成時のL byteと一致しpadはpayloadへ加えない。sourceへのローカルloopbackを生成する処理は対象外（注） |
| 具体値 | 空data、R=1GbpsはM64/wire576bit/occupied672bit。SOF0→EOF576000ps→release672000ps。delay1000psならarrival577000ps。L1500はM1518/wire12208bit/occupied12304bit |

<a id="switch"></a>

## 3. 蓄積交換・egress FIFO・複製

```trace
{
  "id": "spec-ethernet-models#switch",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0150",
    "DIR-REQ-0151",
    "DIR-REQ-0152",
    "DIR-REQ-0153"
  ],
  "upstream": [
    "DIR-FUNC-0035"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| FIFO | output完全パスごとに独立、queue_id=outputパス+`.queue`。offer成功順（処理イベント順、同batchはframe_id順）で保持。送信中は容量から除外、各readyに空きならqueued、満杯ならそのcopyだけdropped(queue_full)。容量0は全copyを破棄、idle直送は対象外（注） |
| phase境界 | phase0はEOF/release/正delay処理完了、phase1はoffer/arrival/0delay処理完了、phase2はdirty outputを完全パス順に評価しidleかつ非空から先頭一件を送信。phase1の満杯判定は同時刻phase2取出しを先取りしない |
| store-and-forward | Switchはarrival（全FCS到達）でreception processingを作り、arrival+forward_delay_psで一回FDB照合。0delayでも到達前送信を許すcut-throughとは区別する。各incoming frame処理は独立でありCPU単一server競合を追加しない |
| FDB | destination unicastの一致entryがあればそのegress一個を選ぶ。入力の対outputならfiltered(reason=same_ingress)でcopy0件。entryなしunicastとbroadcastは入力の対output以外の全接続outputへflood。候補は完全パス順、MAC学習・entry自動更新は対象外（注） |
| 原子的fanout | 全候補の容量を現在の確定状態で判定し、各出力にaccepted又はqueue_fullを記録した一batchをcommit。満杯一個が他の空出力を止めることはない。算術・予約・journal検証失敗なら全候補のbatchを破棄して実行停止。容量不足による通常dropはbatch失敗と区別する |
| 複製の識別 | transfer_id=`frame_id@output完全パス`。非巡回treeとingress除外により同じ元frameは各方向を最大一回通り、一意。source側parent_transfer_id=null、switch出力copyは到着したtransfer_idを親に持つ。reception_idはincoming transfer_id+`@rx` |
| 終端 | 実行イベントはT未満だけ。予定EOF/arrival/処理完了がT以上なら実到達fieldはnull、未完了状態を保持。失敗時は[診断](../診断詳細機能仕様書.md#lifecycle)の確定journalからsnapshot。queue/drop/forwardの状態をfinishで書換える操作は対象外（注） |

<a id="outputs"></a>

## 4. 記録schema・集計・不変条件

```trace
{
  "id": "spec-ethernet-models#outputs",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0155",
    "DIR-REQ-0156"
  ],
  "upstream": [
    "DIR-FUNC-0036"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 共通包絡 | [結果schema2](../結果詳細機能仕様書.md#result-schema)のModelRecordを使用。schema_version=1、request_id=frame_id、origin_request_id=null。time_psは最後に成功した当該行更新時刻。Dは正規非負十進文字列、Sは文字列、?はnull可。以下dataは全field必須、未知fieldはschema不一致 |
| `ethernet.frame` | record_id=frame_id、subject=source。data=`{source:S,src_mac:S,dst_mac:S,ether_type:D,data_hex:S,pad_bytes:D,mac_bytes:D,fcs_hex:S,mac_hex:S,generated_ps:D,ready_ps:D?}`。生成時ready=null、source最初offer時にreadyを設定。frame bytesは生成前cache値で不変 |
| `ethernet.transfer` | record_id=transfer_id、subject=from_port。data=`{frame_id:S,parent_transfer_id:S?,from_port:S,to_port:S,queued_ps:D,sof_ps:D?,eof_ps:D?,release_ps:D?,arrival_ps:D?,planned_eof_ps:D?,planned_release_ps:D?,planned_arrival_ps:D?,status:S,drop_reason:S?}`。status=queued/transmitting/serialized/dropped。queued_psはoffer判定時刻（dropでも保持）、serializedはEOF済、release/arrivalはその各実到達時に更新。SOF前の予定fieldはnull。droppedのreasonはqueue_full、他はnull |
| `ethernet.reception` | record_id=incoming transfer_id+@rx、subject=受信device。data=`{frame_id:S,transfer_id:S,ingress:S,observed_ps:D,ready_ps:D?,planned_ready_ps:D?,status:S,reason:S?,egress_transfer_ids:S[]}`。status=processing/received/filtered/forwarded。到達時だけ作成。endpoint適合はprocessing→received、switchはprocessing→forwarded/filtered、endpoint不適合は即filtered。reasonはdestination_mismatch/same_ingress又はnull、egress列はoutput順でdrop copyも含む。filteredはready=observed（switchはFDB完了時刻）、planned_readyは処理対象で設定し即filterはnull |
| 量・母集団 | ethernet.generatedは元frame数、ethernet.transfer_offeredは全hop copy数、ethernet.queued/transmitting/serialized/droppedはcopyの最終状態数。ethernet.received/filtered/forwarded/processingはreceptionの最終状態数。generatedとreceivedは複製により一致する必要がない。各集計は該当subject別と$allへ一回ずつ計数 |
| 時系列と率 | 各queueのqueue_length/queue_max/queue_mean、各outputのethernet.link_utilization（SOF～release占有時間/区間長）、ethernet.payload_bits（EOFで8L）、ethernet.mac_bits（EOFで8M）、ethernet.wire_bits（EOFで8(M+8)）、ethernet.occupied_bits（releaseで8(M+20)）、ethernet.payload_throughput_bps、各受信endpointのethernet.delivery_ps（received−generated）とethernet.delivery_mean_psを出す。queueと時間/窓/丸め/空集合の規則は結果仕様を共用する |
| 指標登録 | 上記ethernet.*件数はversion1/unit=count/value_kind=integer/sampling=summary/aggregation=sum。bit量はbit/integer/window_summary/sum、利用率は1/number/window_summary/occupancy_ratio、throughputはbit/s/number/window_summary/rate、deliveryはps/integer/point/identity、平均はps/number/summary/sample_mean。delivery点はrequest_idとreceiverを設定、平均のsample_countはreceived数。他は対象output/deviceとnull補助列を使う |
| 保存則 | offered=queued+transmitting+serialized+dropped。arrived copy数=reception行数。forwarded行のegress_transfer_ids件数はそのparentに属すcopy件数（通常treeでは1以上）。filtered/receivedは子copy0、source copyは元frameごとready到達時に一件。serializedは受信済やrelease済と同義ではない |
| 相互参照 | 全copyは一つのframeを参照、各receptionはarrival到達copyに対応。queued≤SOF≤EOF、EOF≤arrival、EOF≤release、processingのready=null、終端receptionのready非null。実到達と予定は一致、停止時予定だけのfieldを実績へ移す処理は対象外（注） |
| 親要件の分担 | DIR-REQ-0145の複合責務のうち、送信frame/linkはDIR-FUNC-0034、有限FIFO・FDB・floodによる交換はDIR-FUNC-0035、入力・結果・停止はDIR-FUNC-0036が担当する。本節の結果は交換節の各copy状態を関連ID付きで保持する |
| 初期状態 | [詳細設計の初期state表](../../design/Ethernetモデル詳細設計書.md#prepare)のEndpoint/Switch/Linkの全fieldをmetadataへcanonical JSONで保存する。全queue空・方向idle、generator ordinal0。設定生成列はworkload snapshotを正本とする |
| 実入力・確認先 | [検証仕様](../../verification/cases/Ethernetモデル検証仕様書.md)と[fixtures](../../verification/fixtures/ethernet/scenarios.json)。詳細な所有状態・payload・失敗処理は[詳細設計](../../design/Ethernetモデル詳細設計書.md) |
