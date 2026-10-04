# Ethernet媒体拡張詳細設計書

文書バージョン：`1.1.2`
対象GitHubバージョン：`main @ 7bb9bfc`
予定公開版：`v1.1.2`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-04` | 半二重CSMA/CD・固定BEB・T1 pipelineの内部実装と検証範囲を接続し、v1.1.2向け文書版を確定 |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。10/100半二重CSMA/CDと1000BASE-T1全二重媒体を定義 |

文書ID：`design-ethernet-media`

| 項目 | 内容 |
| --- | --- |
| 担当元 | [architecture#arch-ethernet-media](../アーキテクチャ設計書.md#arch-ethernet-media)を実現する。契約は[媒体仕様](../specs/models/Ethernet媒体拡張詳細機能仕様書.md)、既存frame/FDBは[v1設計](Ethernetモデル詳細設計書.md)を正本とする |
| 状態 | 設計確定。開発中ソースに製品実装を追加。製品試験の実施範囲は本書の検証記録又は対応する検証仕様を参照。規格全体適合・実機試験は未実施。文書版を1.1.2に統一し、公開版はv1.1.2向けPRとして準備する。 |

<a id="half-state"></a>

## 1. 媒体対とMAC・試行の状態

```trace
{
  "id": "design-ethernet-media#half-state",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0161",
    "DIR-REQ-0162",
    "DIR-REQ-0163",
    "DIR-REQ-0164",
    "DIR-REQ-0165",
    "DIR-REQ-0166",
    "DIR-REQ-0168"
  ],
  "upstream": [
    "architecture#arch-ethernet-media"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 要素・処理 | 所有・不変条件 |
| --- | --- |
| MediaFabric | v1のFrame/Transfer/Reception、FDB、queueにMediaPairMapとAttemptMapを追加。各物理対が二端MACと信号区間を所有し、pair単位で遷移する。root coordinator内部ID=`@profile:ethernet.l2.store-forward.v2:<rootpath>`。全simple/channel構築・initialize後に初期journal/dispatcherを生成する |
| PairTimeline | pairごとにBTreeMap<時刻,論理変更配列>を持つ。各変更はchange_id/kind/attempt_id/generationを持ち、coordinator所有の最早時刻TimerToken一つだけを物理予約する。特定attemptの取消しは一致する論理変更だけを削除し、同時刻の他端EOF/IFGを維持する。最早時刻が変わった場合だけ所有tokenを取消して再予約する。callbackは当該時刻集合を取り出し遷移後の最早を予約する |
| PairState | id、a/b、R/P、mode、duplex、端別PHY、signal interval/token、dirty、次boundary token。各MacStateはcurrent transfer?、FIFO、local_busy、idle_since?、ifg_ready、attempt generation、backoff deadline?、canceled-token集合を持つ。初期local_busy=false/ifg_ready=true/current=null/FIFO=[]、idle_since=nullは時刻0以前のidle認証を表す |
| 準備 | pair一意/対向/速度/role/遅延境界をchecked u128で検査。全設定と不変frame計算が成功してから構築。halfの2P+jam<slot、max(2P,64B)+jam<slotを確認し遅延値の診断へ両辺を含める |
| PairArbitrate、phase2 | 両端の開始前状態を読み、全許可端を収集。currentがあるretryを優先、なければFIFO先頭。両端の時刻/メモリ/予約を検証後一batchでAttempt作成・queue移転・SOF・peer SignalOn・EOF等を登録。dirty対象は`@media:<id>`の安定文字列IDで、coreへCAN等の比較キーを追加しない |
| PairBoundary、phase0 | 同時刻の信号開始/終端と局所EOF/jam終了/IFG/backoff期限を一callbackにまとめる。半開区間の終了を除き開始を加えた新carrierからcollisionを評価し、collision対象のEOFを無効化した後に残りEOFを確定。新carrier=trueは同時刻IFG満了より優先して許可を取消す。最後にidle変化とdirtyを反映 |
| 衝突 | 該当attemptをjammingへ更新し予定正規EOF/releaseの論理変更だけを削除する。正常Arrivalはまだ作られていない。既存SignalOffを取消しjam_endとpeer側jam_end+Pへ置換。相手端のcollisionは相手carrier到達時に別に確定し、peer検出時刻を自分の検出時刻へ短縮する処理は対象外（注） |
| 取消し | TimerTokenはcontextが所有し、cancelと状態deltaを同batchへ置く。cancel済項目はFESの論理残件/処理数から除外。attempt generationをIDとは別にchecked増分し、各AttemptMap行へ不変値として保存、新タイマはその値をpayloadへ付ける。ArrivalはEOF成功後にだけsend_atし、TimerTokenを返さないため取消し対象と呼ばない。配送前に参照attempt serialized/不変世代を照合し、次frameのSOFが進めたport現在世代は参照しない |
| jam終了 | attempt.collided/jam_endをcommitしn16ならtransfer dropped、current=null。n<16ならSHA-256計算→r/deadlineを記録、transfer backoff、期限timer予約。r0ならphase0完了内で期限到達済みを確定しdirty、再帰的な即時送信は行わない（注） |
| 配送権限 | coordinatorはprepareで当該FabricContextと選択profile内の解決済output handle集合へ束縛する。send_atはhandleの論理port/schema/宛先を照合し、送信元は実deviceのoutput、self_idはcoordinatorを保持する。通常simple callbackは自己portのみ。任意の他model portを文字列探索して配送する処理は対象外（注）。PairBoundary tokenとdirty media資源の所有者はcoordinator |
| キャリア待ち | busy→idleでifg_ready=false、q+96BのPairBoundaryを予約。新busyで予約を取消す。IFG満了でtrue。backoff deadlineより前はfalse扱い、期限後carrierが残ればdeferred。初回deferredはFIFO、retry deferredはcurrentで区別する |
| 成功 | EOFでattempt/transfer serialized、MAC既定release予約を維持し、この成功EOFで初めてArrivalをsend_at予約する。releaseでcurrentを解放。半二重peer carrierはEOF+Pで終端し、peer側IFGはそこから開始。FCS正常配送を一回だけ許可 |
| FIFO | offerの容量はFIFOだけ。currentにあるretryを再offerしない。満杯通常dropは親copyを保持し他出力へ伝播させない。後続sourceもswitchcopyも同じ規則で扱う |
| 原子性 | 各callbackは更新差分・取消し・予約・記録を検証後commit。失敗はE-0002/E-0004として停止し、model内部が変更済みでも結果は直前journalだけ。media全体をcloneして継続rollbackする処理は対象外（注） |

<a id="t1-state"></a>

## 2. full/T1 pipeline・型付き通知

```trace
{
  "id": "design-ethernet-media#t1-state",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0169",
    "DIR-REQ-0170",
    "DIR-REQ-0171",
    "DIR-REQ-0172"
  ],
  "upstream": [
    "architecture#arch-ethernet-media"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 要素 | 実現方法 |
| --- | --- |
| full branch | v1方向別MAC送信状態を使い、media対のcarrierをeligibilityへ接続しない。常にattempt1、collision_count0。PHY端のTX/RXはimmutable設定、役割は静的属性 |
| 算術 | MAC EOF/releaseはSOF原点、MDI予定とarrivalはTX+P+RXでchecked加算。MAC current解放はreleaseだけ。PHYpipeline内に複数frameが重なっても定数delayで順序が保持されるため別の有限PHYキューを作る処理は対象外（注） |
| Arrival | 参照AttemptMap行の不変generation一致・serialized・未到達を検証してtransfer/attempt arrivalを設定しv1 Receptionを作成。PHY位相だけのtimerは生成しない。MDI欄は計算された予定として保持する |
| Link snapshot | LinkV2.state=`{bitrate_bps:D,delay_ps:D}`。phy_link静的行は初期化全件成功時に作る。duplex half/fullの実行分岐とrole/latencyの値域はprofile validatorで固定しruntime中に再解釈しない |

| Kind | 非null body field・phase・宛先 |
| --- | --- |
| SourceReady | physical_link,frame_id,output、generation=0、phase1・source。transfer/attempt/reception=null |
| PairBoundary | physical_link、generation=pair予約世代、他ID全null、phase0・coordinator。予約時刻の全端変化をstate台帳から取り出す |
| Arrival | physical_link,frame_id,transfer_id,attempt_id,output、generation=attempt世代、reception=null、phase1・宛先device |
| ProcessComplete / ProcessReady | physical_link,frame_id,reception_id、generation=0、他IDnull。完了0→処理1・reception owner、0delayは直接ProcessReady1 |
| Generate / PairArbitrate | 共通dispatcher/dirty resourceの内部descriptorを使う。前表codecへ二重登録しない（注） |

<a id="journal"></a>

## 3. journal・観測・停止と互換性

```trace
{
  "id": "design-ethernet-media#journal",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0167",
    "DIR-REQ-0168",
    "DIR-REQ-0172",
    "DIR-REQ-0173"
  ],
  "upstream": [
    "architecture#arch-ethernet-media"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 処理 |
| --- | --- |
| ModelRecord | 仕様の5登録schemaへ完全fieldをupsert。attempt1行はSOFで作成、初期frame/receptionのshapeはv1。PHY行は初期化時0、その他time_psは当該行の最後のcommit時刻。共通キーsort/一意性を適用する |
| 初期metadata | Endpoint/Switchはv1初期stateを使う。coordinator state=`{profile:"ethernet.l2.store-forward.v2",deference_policy:"continuous-idle-96.v1",backoff_policy:"sha256-beb.v1",seed:D,pairs:[{id:S,current_a:null,current_b:null,local_busy_a:false,local_busy_b:false,ifg_ready_a:true,ifg_ready_b:true,next_generation:"0"}],attempts:[]}`。pairsはid順。構造moduleは{}。設定/役割/遅延はmodel-config snapshotとphy_linkに保持 |
| 積分 | 送信区間は正常SOF～EOF又はSOF～jam_end。jam_startが未来でも計測器へ予定だけで量を加えず、[jam_start,min(H,実jam_end又は未到達時planned_jam_end))の非負長から求める。局所送信区間とjam区間を保存し窓境界で分割する。送信中の区間終端はHでclipし、予定時刻の積分利用から実EOF/jam_endを生成しない。queue_meanはcurrentを除く |
| 終了 | 未到達EOF/arrivalはnull、PHY予定欄は予定のまま保持。T以後のjamやretryは実績へ繰り上げない。finishはtoken/context/frame共有参照の解放のみで新規計測を出す対象外（注） |
| schema互換 | serializer/FDB/旧codecの実装を共用できるが登録entryはv1/v2で独立する。v1のtransfer/1をv2の拡張field入りobjectで上書きしない。v2のdescriptor集合は出力metadataへ全件明示する |
| 検証 | [媒体検証](../verification/cases/Ethernet媒体拡張検証仕様書.md)の0033～0039。carrier同時刻、preamble/jam、BEB固定vector、取消し、T、T1非対称遅延、混在treeとv1回帰を分担する |

## 開発中実装の境界

製品コードは共通INI/NED・結果公開層に専用input/runtime/outputを接続する。媒体対の論理timeline・予約世代・試行状態、CAN FDの不変frame cacheと受信状態をモデル内部で所有し、callbackの時刻・予約を事前検証してから結果へ反映する。公開ProfileRegistry、汎用Context/Envelope/TimerToken APIとモデル共通arenaは未実装であり、上記設計の公開拡張APIへの適合完了を示さない。

CAN FDは外部位相bit数による時間評価で、結果に`externally-precomputed-phase-bits`と`structural-only`を保存する。媒体v2は連続idle96bitのdeferenceと固定SHA-256 BEB、T1は時刻0からlink-up済みの固定PHY遅延を採用する。内容依存FD CRC/stuffing、波形、training、規格全文適合はこの実装の範囲外。
