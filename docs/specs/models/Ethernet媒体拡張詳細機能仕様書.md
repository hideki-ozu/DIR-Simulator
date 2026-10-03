# Ethernet媒体拡張詳細機能仕様書

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。10/100半二重CSMA/CDと1000BASE-T1全二重媒体を定義 |

文書ID：`spec-ethernet-media`

| 項目 | 契約 |
| --- | --- |
| 状態 | 仕様・設計・解析fixtureを確定。製品実装・製品試験は未実施 |
| 基盤 | `ethernet.l2.store-forward.v2`を明示選択する。[v1仕様](Ethernetモデル詳細機能仕様書.md)のframe/FCS/FDB/tree/処理遅延を再利用し、本書が媒体アクセスと結果差分を定義する。v1入力・出力を保持する |
| 忠実度 | MAC時間・信号の伝搬区間・衝突・再送・固定PHY遅延の離散イベントモデル。注：波形、符号化/FECのbitstream、規格全文適合認証、実機誤り率、10BASE-T1S/PLCAは対象外 |

## Ethernet親要件に加える媒体の分担

本書は[Ethernet L2通信](../../要件定義書.md#dir-req-0145)のうち、半二重と1000BASE-T1の子要件を具体化する。v1のframe・交換契約を再利用し、媒体アクセスとPHY観測の差分をv2で選ぶ。提供は1.0.0以後の将来対象である。

| 確認する経過 | 本書の担当節 | v1・共通基盤との境界 |
| --- | --- | --- |
| 半二重の構成から再送まで | [half-config](#half-config)→[csma](#csma) | 物理対と伝搬境界を準備検証し、carrier、IFG、衝突、jam、BEB、試行上限を確定する。同じtransferの再試行として保持し、正常frameのFCS/FDB規則はv1を使う |
| T1の構成から受信まで | [t1](#t1)→[records](#records) | 1Gbps全二重とmaster/slave、TX/RX PHY値を確定し、MAC EOFから方向別arrivalを計算する。PHY遅延は受信へ加え、MAC releaseと方向独立性は保持する |
| Switchと終了結果 | [records](#records) | 成功arrivalだけがreception・Switch交換へ進む。copyとattemptの母数を分け、衝突途中・jam途中・backoff待ちを共通[0,T)と確定journalで保存する |

半二重で衝突したattemptは信号占有とjamを残す一方、正常serializedやreceptionを増やさない。再送で成功したcopyは一件のtransferに複数attemptを持つ。ある物理対の衝突が別のfull/T1対を止めないことも、Switchを含む親要件の連携確認に含める。

[DIR-AC-0039](../../要件定義書.md#dir-ac-0039)のcarrier/jam、[DIR-AC-0040](../../要件定義書.md#dir-ac-0040)のBEB/終了、[DIR-AC-0041](../../要件定義書.md#dir-ac-0041)のT1時刻、[DIR-AC-0042](../../要件定義書.md#dir-ac-0042)の媒体独立・v1回帰を選択profileに対応付けて確認する。

<a id="half-config"></a>

## 1. 配置・版付き入力・媒体の準備

```trace
{
  "id": "spec-ethernet-media#half-config",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0161",
    "DIR-REQ-0166",
    "DIR-REQ-0168"
  ],
  "upstream": [
    "DIR-FUNC-0039"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 機能と型 | [DIR-FUNC-0039](../../機能仕様書.md#dir-func-0039)。`dir.ethernet.EndpointV2/SwitchV2/LinkV2`は別の固定登録キー。Endpoint/Switchのパラメータ・scalar gates、Linkのbitrate/delayはv1と同じ。protocol=`dir.ethernet.media`、message=`Arrival`、schema=`dir.ethernet.media.Arrival`版1。Link capability=`ethernet-media-link`版2。旧descriptorを実行中に変更する処理は対象外（注） |
| 選択 | INI General `model-profile = "ethernet.l2.store-forward.v2"`、`model-config = "model.json"`。workloadは共通schema2、kind=`ethernet.explicit.v1`とframe入力を維持。相対パスはINI親。v1 model-configをv2へ暗黙変換する処理は対象外（注） |
| root | 全キー必須`{schema_version:2,endpoints:EndpointConfig[],switches:SwitchConfig[],seed:D,physical_links:PhysicalLink[]}`。EndpointConfig/SwitchConfigはv1そのまま。seedは0..u64maxの正規十進文字列。全階層の未知/欠落/重複キー・型違反を準備失敗とする |
| PhysicalLink | 全キー必須`{id:S,a:S,b:S,phy_mode:S,duplex:S,a_phy:PhyEnd,b_phy:PhyEnd}`。idはASCII識別子で実行内一意、a/bは対向するoutput完全パス、UTF-8辞書順a<b。各outputを一回だけ含め、NEDのtx/rx peer対と一致させる。PhyEnd=`{role:S,tx_latency_ps:D,rx_latency_ps:D}` |
| PHY選択 | phy_mode=`10base-t`はR=10000000、`100base-tx`はR=100000000、どちらもduplex=`half`又は`full`。これらのPhyEndはrole=`none`、TX/RX latency=`0`。`1000base-t1`は第3節のfullのみ。その他名・速度・duplex組合せは準備失敗 |
| 対向構成 | 二つのLinkV2は同bitrate/同delayで一つの物理対を表す。各物理対のMACはちょうど二つ。Switchを介して複数の半二重・全二重対をtree接続できる。注：hub/repeater・multi-drop共有同軸・複数MAC一衝突領域は本版対象外 |
| half伝搬境界 | P=片方向Link.delay、B=10^12/R ps/bit（10/100Mbpsは整数）。`2*P+32*B < 512*B`を広い整数で検証する。P=0も受理。衝突検出の最遅2Pとjam32bitをslot内へ収め、preamble内検出時の追加完了でもmax(2P,64B)+32B<512B。100MbpsはP<2400000ps、10MbpsはP<24000000ps |
| half固定定数 | slot=512B、preamble/SFD=64B、jam=32B、IFG=96B、最大試行16、BEB指数上限10。deference_policy=`continuous-idle-96.v1`、backoff_policy=`sha256-beb.v1`をmodel state/PHY行へ保存。定数上書きキーは準備失敗（注） |
| 準備順 | 共通NED/INI→v2 config/workload schema→全device列挙とtree/peer対→physical pair/PHY/速度/role/伝搬境界→MAC/FDB/frame cache→初期化。不正はE-0001の共通profile診断、details.rule/targetに違反規則・対象link/portを保存する |
| 独立領域 | 衝突・carrier・backoffは物理対に閉じる。Switchは成功frame全体が到達して処理遅延を終えてから各egressへcopyを生成する。あるhalf対のretryは別のfull/T1対の送信状態を変更しない |

<a id="csma"></a>

## 2. carrier・衝突・再送・キュー

```trace
{
  "id": "spec-ethernet-media#csma",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0162",
    "DIR-REQ-0163",
    "DIR-REQ-0164",
    "DIR-REQ-0165",
    "DIR-REQ-0166"
  ],
  "upstream": [
    "DIR-FUNC-0039"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 状態・条件 | 確定した振る舞い |
| --- | --- |
| 送信区間 | sof=sはpreamble開始。成功なら発光/送信信号の局所区間[s,eof)、衝突なら[s,jam_end)。peerでのcarrierはその区間をPだけ平行移動した半開区間。local_busyは自分の送信信号又はpeerの到達carrierがあるときtrue。fullではこのcarrierを送信許可に使う処理は対象外（注） |
| carrier待機 | 初期は時刻0以前にIFGを満たしたidle。以後local_busyがfalseへ変わった時刻qから96B連続idle後に送信可能。途中のtrueでIFG待機を取消し、次のfalseから数え直す。queued又はretry対象があり、backoff期限到達済みで、IFG許可時にphase2でSOFを確定 |
| 採用するdeference | 本profileは連続idle96bitを固定選択する。注：IEEEの二段階IFGで後段のcarrier変化を無視する実装との差を持ち、全Clause4動作への適合を表明しない。将来の厳密deferenceは別policy/profile版で追加する |
| 同時開始 | 媒体pair単位のphase2で両端のcarrier/IFG/期限を同じ開始前snapshotから評価し、許可された両端を同batchで開始する。P>0で相手carrier未到達なら後着開始も可能。単なる出力path順で片側を先行勝者にする処理は対象外（注） |
| 衝突検出 | localがtransmittingでpeer carrierが立ち上がるとその時刻にcollisionを確定。初回だけ計上。二つのSOF=sA,sBが検知前に重なればAの検出=max(sA,sB+P)、Bはmax(sB,sA+P)。各検出時刻が各局所送信区間内にあるときだけ適用。jam中の追加carrierは衝突回数を増やす対象外（注） |
| jam | collision_detected=cならjam_start=max(c,s+64B)、jam_end=jam_start+32B。preamble/SFD中の衝突も64bitを完了してから32bit jamを送る。cで通常データ完了を無効化し、jam_startまでに再検出した衝突は同じ試行に合流。jam内容は抽象固定patternとして扱い、FCS付き正常frameを生成する対象外（注） |
| 無効化 | 衝突試行の予定EOF/releaseを取消し、正常Arrivalを新規生成する資格を無効化して、通常のserialized/payload量/receptionを発生させない。既に予定したpeer carrier終端をjam_end+Pへ置換。実送信したfragment/jamはattemptと信号時間へ記録する。Arrivalは成功EOF後だけsend_atで作成し、それ以前の衝突に取消し対象Arrivalは存在しない。正常受信は参照AttemptMap行の不変generationと一致する通知だけを許可（portの現在世代やcurrentは比較対象外） |
| BEB | n=当該copyの衝突済み試行数（初回1）。n<16ならk=min(n,10)、r∈[0,2^k−1]を下の固定算法から選び、`backoff_until=jam_end+r*512B`。時間はcarrier busy中も経過する。注：802.11のようなbusy時の残slot凍結は採用対象外 |
| seed算法 | `UTF8("dir.ethernet.beb.v1") + 00 + ASCII(seed) + 00 + UTF8(transfer_id) + 00 + ASCII(n)`のSHA-256を計算（+はbyte列連結、00は1byteのNUL）。digest先頭8byteをunsigned big-endian Uとし、r=U AND (2^k−1)。seed/nの文字列は先頭0なし。独立streamはtransfer_idに含まれるoutputパスで区別し、イベント順やhash table順に依存しない。意図は再現可能な疑似乱数であり暗号安全性の評価は対象外（注） |
| retry時刻 | backoff_untilを経過してもcarrier又はIFGが未達ならdeferred。idleが続く場合はmax(backoff_until,q+96B)で次SOF。backoff終了をIFGの開始とみなして96Bを二重に加える処理は対象外（注）。SOFごとにattempt numberを1増やす |
| 試行上限 | n=16のjam終了でcopy=dropped/drop_reason=attempt_limit。17回目のSOFと16回目後の乱数抽選は行わない（注）。成功copy及び次のcopyは衝突数0から始める |
| FIFO・容量 | v1のoutput別FIFO・queueCapacity/drop-newestを維持。初回SOFで先頭を除去してcurrentへ所有移転し、retry中も同copyをcurrentに保持。後続は追越さず、currentは送信中/jam/backoff/deferredのどれでも待機容量外。初回SOF前のdeferredはFIFO内なので容量へ含める |
| current解放 | 成功EOF後は自MAC IFG終了release=eof+96Bでcurrentを解放。再送上限dropはjam_endでcurrentを解放するが、carrier/IFG規則が次copyを制限する。releaseは自MACのIFG終了でありpeer carrierにより次SOFがさらに遅れる場合もある |
| same-time境界 | 信号区間は[begin,end)。同時刻の全carrier開始/終端・衝突・EOF判定をpair単位phase0で一括評価。区間終端でのcarrier開始は終了した送信に衝突を追加しない。phase1 offerの後、phase2で許可判定。P=0の両端SOFは次deltaのphase0でcollisionを確定 |
| T・失敗 | [0,T)だけ実行。collision=Tは未検出transmitting、jam_end=Tはjamming、backoff期限=Tはbackoff、Arrival=Tは未受信。失敗時は成功commit prefixだけを保存し、終了時の強制再送・成功補完は対象外（注） |

<a id="t1"></a>

## 3. 1000BASE-T1とfullの時間

```trace
{
  "id": "spec-ethernet-media#t1",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0169",
    "DIR-REQ-0170",
    "DIR-REQ-0171",
    "DIR-REQ-0172"
  ],
  "upstream": [
    "DIR-FUNC-0040"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 機能 | [DIR-FUNC-0040](../../機能仕様書.md#dir-func-0040)。phy_mode=`1000base-t1`、duplex=`full`、両方向R=1000000000だけを受理する。10/100Mbps又はhalf指定は準備失敗（注） |
| 役割 | a_phy/b_phyのroleをmaster/slaveのいずれかとし、一対に一つずつ必須。masterはクロック源、slaveは回復クロック使用という構成属性。役割からMAC優先度・仲裁・片方向送信権を生成する処理は対象外（注） |
| link-up | 全物理対は時刻0で確立済みup、実行中固定。注：training、自動ネゴシエーション、再同期、link障害、master選出競合は対象外。両master/両slave/noneは準備診断で拒否する |
| PHY遅延 | tx_latency_ps/rx_latency_psは各端の0..u64max整数Dで明示必須。校正して使う値であり、全PHY共通の既定実測値は設定しない（注）。0は理想PHY、実PHYを表現する値とは区別してmetadataへ残す |
| MAC時刻 | v1と同じwire_bits=8(M+8)、occupied_bits=8(M+20)。MAC sof=s、eof=s+ceil(wire_bits*10^12/R)、release=s+ceil(occupied_bits*10^12/R)。全二重は方向ごとに独立し同時SOFを受理する |
| 物理・受信時刻 | from=i,to=jとしてmdi_sof=s+TX_i、mdi_eof=eof+TX_i、peer_mdi_sof=mdi_sof+P、peer_mdi_eof=mdi_eof+P、arrival=eof+TX_i+P+RX_j。arrival後のEndpoint.rxProcessingDelay又はSwitch.forward_delayをさらに一回加算する |
| 遅延の責務 | TX/RX PHYは一定遅延pipelineとしてMAC時間軸を平行移動する。MACbusy・release・次SOFへPHY遅延を追加せず、PHY区間だけを時間移動する。注：750MBdやFEC符号量で1GbpsのMAC frame時間を再除算する処理は対象外 |
| classic full | 10base-t/100base-txのfullはPHY遅延0、v1と同じ方向別転送。carrier/jam/BEBを適用しない。v2のattemptは一成功試行で表す |
| 算術 | 全加算・乗算をchecked u128で計算し、予約が必要な時刻のu64値域を検査する。範囲超過はE-0004、当該batchの未commit効果を公開する対象外（注） |
| 空payload例 | MAC64byte、R1Gbps、P1000ps、A TX100000/RX400000、B TX300000/RX200000。両SOF0、両EOF576000、release672000。A→B arrival877000、B→A arrival1277000ps |

<a id="records"></a>

## 4. payload・結果・計測・互換性

```trace
{
  "id": "spec-ethernet-media#records",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0167",
    "DIR-REQ-0168",
    "DIR-REQ-0172",
    "DIR-REQ-0173"
  ],
  "upstream": [
    "DIR-FUNC-0041"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 正確な契約 |
| --- | --- |
| 機能と包絡 | [DIR-FUNC-0041](../../機能仕様書.md#dir-func-0041)。[共通結果schema2](../拡張モデル共通詳細機能仕様書.md#results)を使用。Dは非負正規十進文字列、Sは文字列、?はnull可。dataは全field必須、未知field拒否。request_id=frame_id、origin_request_id=null、time_ps=最後の行更新時刻（PHY行0） |
| 登録集合 | `ethernet.frame`/1、`ethernet.reception`/1、`ethernet.transfer`/2、`ethernet.attempt`/1、`ethernet.phy_link`/1。frame/receptionのdataはv1全fieldをそのまま使う。receptionは成功copyの全体到達だけで生成 |
| transfer/2 | record_id=frame_id@output、subject=from_port。dataはv1 transfer全fieldに`physical_link:S,attempt_count:D,collision_count:D,last_attempt_id:S?,backoff_until_ps:D?`を追加。statusはqueued/deferred/transmitting/jamming/backoff/serialized/dropped。drop_reasonはqueue_full/attempt_limit又はnull |
| transfer時刻 | queuedは初回offer、sofは最新試行の開始（未試行null）、eof/release/arrivalは成功試行だけの実績。予定EOF/release/arrivalは有効な最新試行の予定、衝突確定でnullへ戻す。初回SOF前のlast_attempt=null、試行後は最新attempt ID。backoff_untilは直近抽選期限を保持し、次SOFでnullへ戻す。serializedのattempt_countは1以上 |
| attempt/1 | record_id=`transfer_id#attempt番号`（1始まり）、subject=from_port。data=`{transfer_id:S,physical_link:S,from_port:S,to_port:S,number:D,sof_ps:D,planned_eof_ps:D,planned_release_ps:D,planned_arrival_ps:D,collision_ps:D?,planned_jam_start_ps:D?,planned_jam_end_ps:D?,jam_end_ps:D?,eof_ps:D?,release_ps:D?,arrival_ps:D?,backoff_slots:D?,backoff_until_ps:D?,status:S,planned_mdi_sof_ps:D,planned_mdi_eof_ps:D?,planned_peer_mdi_sof_ps:D,planned_peer_mdi_eof_ps:D?}` |
| attempt状態 | SOFでtransmitting、衝突でjamming、jam終了でcollided、成功EOFでserialized。planned_jam_start_psは衝突時に確定した予定で、T前到達の実績とは区別する。planned_jam_endも予定、jam_endは実到達のみ。衝突前の予定EOF等は参考として不変保持し、その到達を正当化する値にしない（注）。n<16の抽選はjam終了時に設定、n=16のbackoff両欄はnull。成功はcollision/jam/backoff欄全てnull |
| PHY実績欄 | planned_mdi_sof_ps/planned_peer_mdi_sof_psはSOF時に算出した予定。planned_mdi_eof_ps/planned_peer_mdi_eof_psは成功EOFで確定した予定値、衝突はnull。これらはPHY個別イベント実行時刻でなく平行移動計算値であることをfield定義とmetadataに保持し、arrivalだけが宛先へ実到達した時刻。halfはTX/RX0で同式を使う |
| phy_link/1 | record_id=PhysicalLink.id、subject=`@media:<id>`、request_id=null。data=`{a:S,b:S,phy_mode:S,duplex:S,bitrate_bps:D,propagation_ps:D,a_phy:PhyEnd,b_phy:PhyEnd,link_state:"up",seed:D,deference_policy:S,backoff_policy:S}`。halfのpolicyは第1節、fullは両方`not-applicable`。初期化全成功後の静的行、データ更新は対象外（注） |
| 通知codec | portのArrival及び内部schema`dir.ethernet.media.<Kind>`版1はcompact UTF-8 JSON・キー辞書順。共通body=`{kind:S,physical_link:S,frame_id:S?,transfer_id:S?,attempt_id:S?,generation:D,output:S?,reception_id:S?}`。Arrivalは全IDのうちframe/transfer/attempt/outputが必須、reception=null。受信Envelopeの宛先と予定arrivalを検証し、FrameMap不変bytesを参照する。内部Kindsとfieldの指定は設計を正本とする |
| payload検証 | schema/kind/型の不正はE-0002 invalid_event。参照・所有者・状態・世代の不一致はmodel_failed。通常の取消済FES項目は核がdispatch前に除去し、callbackを呼ぶ対象外（注）。世代を無視して不正frameを配送する処理は対象外 |
| 保存則 | offered=queued+deferred+transmitting+jamming+backoff+serialized+dropped。copyのattempt_count=attempt行数、collision_count=collision_ps非null行数、reception数=成功arrival数。frame数・copy数・attempt数を別母数で保持。Switchの親子はv1と同じtree関係 |
| 指標方針 | v2では下表のmetric集合を厳密に登録する。v1の全指標を暗黙追加する処理は対象外（注）。詳細frame量・受信遅延は台帳から再構成可能。窓/summary、H、空集合、partial prefixの共通規則を適用 |
| v1互換 | v1選択時は元の3schema版1・descriptor・入力制約・CRC・時刻・計測集合をそのまま使用する。v2追加の旧descriptor書換えとv1へのrecord追加は対象外（注）。v1回帰fixtureを同じbuildで実行する |

| metric ID | version / unit / value_kind / sampling / aggregation | 対象・値・補助列 |
| --- | --- | --- |
| `ethernet.media.attempts`, `ethernet.media.collisions`, `ethernet.media.retry_exhausted` | 1 / count / integer / summary / sum | 各outputと$all。SOF件数、検出件数、attempt_limit copy数。0件も出力。request_id/receiver/reason/sample_count=null |
| `ethernet.media.queue_length` | 1 / count / integer / point / identity | target=output.queue、初期0とoffer/取出し時の待機長。満杯で不変も出力。原因frameのrequest_id、初期null。他補助列null |
| `ethernet.media.queue_mean` | 1 / count / number / summary / time_mean | target=output.queue、待機長の[0,H)積分/H。H0はnull。補助列null |
| `ethernet.media.tx_utilization` | 1 / 1 / number / window_summary / occupancy_ratio | target=output、局所送信信号区間の和集合/H又は窓長。進行中はSOFからHまでを含める。jamを含みIFG/backoffを含めない。PHY遅延はMAC原点へ足さない。正長idle0、H0 null |
| `ethernet.media.jam_ps` | 1 / ps / integer / window_summary / sum | target=output、[jam_start,実jam_end又は未到達時planned_jam_end)と観測区間の非負交差長。T途中はHでclipして未完了分を積分するが実jam_endはnullを保持。0も出力 |
| `ethernet.media.backoff_slots` | 1 / count / integer / point / identity | target=output、jam終了で抽選したr。request_id=frame_id、reason/receiver/sample_count=null。n16には抽選点なし |
| `ethernet.media.delivered` | 1 / count / integer / summary / sum | 各Endpointと$all、receivedの件数。0も出力。補助列null |

| 計測共通 | 契約 |
| --- | --- |
| 時刻列 | pointはtime_psのみ、summaryはstart=0/end=Hのみ、窓はstart/endのみ。点event_seq/effect_seqは実callback、初期/集計はnull。登録versionは文字列`1`、descriptorを名前順保存。FIFO currentはqueue_lengthから除く |
| 再構成 | 各Endpoint配送の経路遅延はreception.ready−frame.generated。PHY設定とattemptからMAC/MDI/arrival差を分離。衝突したattemptをpayload成功量に加える処理は対象外（注） |
| 確認先 | [詳細設計](../../design/Ethernet媒体拡張詳細設計書.md)、[検証仕様](../../verification/cases/Ethernet媒体拡張検証仕様書.md)、[fixture一覧](../../verification/fixtures/ethernet-media/scenarios.json) |

## 5. 一次資料と採用判断

| 資料 | 確認内容と限界 |
| --- | --- |
| [IEEE公開Clause4改訂案 Table4-2](https://www.ieee802.org/3/as/public/0503/4d0_1_CMP.pdf) | 10/100Mbpsのslot512、jam32、IFG96、試行16、指数10を確認。公開草案という資料状態を区別する |
| [IEEE 802.3作業部会のbackoff解説](https://ieee802.org/3/SPEP2P/email/msg00141.html) | 打切り二進指数backoffとcapture effect。公平性を保証するモデルへ置き換える処理は対象外（注） |
| [Microchip MAC送信仕様](https://onlinedocs.microchip.com/oxy/GUID-2ACDA668-0A87-46A1-B7FC-9DC74A5461AD-en-US-3/GUID-ACF3FBFC-B236-4E46-B1A7-DDD0E2EEA442.html) | preamble/SFD途中の衝突でも当該領域を終えてjamを送る動作を確認。特定実装の全レジスタを採用する主張とは区別 |
| [IEEE公開CSMA/CD解説資料](https://www.ieee802.org/3/cg/public/Sept2017/Beruto_3cg_01a_0917.pdf) | IEEE deferenceの二段階IFGと本profileの連続idle96の差を明示するため参照。提案PLCAやT1Sを本profileへ取り込む資料ではない（注） |
| [IEEE 802.3bp承認情報](https://www.ieee802.org/3/bp/)・[公開97.1.2資料](https://www.ieee802.org/3/bp/public/may15/tu_3bp_02a_0515.pdf) | 1000BASE-T1の1Gbps全二重・master/slaveクロックを確認。MAC優先権と区別する |
| [TI DP83TG720S-Q1データシート](https://www.ti.com/lit/ds/symlink/dp83tg720s-q1.pdf) | TX/RX latencyがPHY・MAC interface等で異なるため明示校正値を採用。特定製品値を万能defaultにしない（注） |
| プロジェクト決定 | 2026-09-29に確認。2MAC物理対、continuous-idle-96、SHA256による再現可能BEB、固定up・固定PHY遅延は評価profileの選択。全IEEE 802.3の実機適合を証明する仕様とは区別する |
