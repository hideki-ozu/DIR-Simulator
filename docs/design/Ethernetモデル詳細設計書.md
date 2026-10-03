# Ethernetモデル詳細設計書

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：EthernetFabricContext・CRC・phase遷移・journalの内部契約を確定。親要件の交換責務を追跡し、共通profile・codec登録境界と照合。CSMA/CD半二重・1000BASE-T1の媒体別profileと互換境界を追加 |

文書ID：`design-ethernet`

| 項目 | 内容 |
| --- | --- |
| 適用 | 本書の基準契約はethernet.l2.store-forward.v1。半二重と1000BASE-T1はv2の[媒体設計](Ethernet媒体拡張詳細設計書.md)を併用 |
| 状態 | 仕様・設計・検証入力を具体化。製品実装・製品試験は未実施 |

<a id="prepare"></a>

## 1. 登録・準備・所有境界

```trace
{
  "id": "design-ethernet#prepare",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0146",
    "DIR-REQ-0148",
    "DIR-REQ-0154"
  ],
  "upstream": [
    "architecture#arch-ethernet"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 詳細設計 |
| --- | --- |
| 根拠仕様 | [profile](../specs/models/Ethernetモデル詳細機能仕様書.md#profile)、[wire](../specs/models/Ethernetモデル詳細機能仕様書.md#wire) |
| 登録 | EthernetProfile descriptorはprofile名、Endpoint/Switch/Linkのschema、ethernet.explicit.v1 generator、ethernet.*の結果/指標schema、kind別Ethernetイベントschemaと共有codecを登録。同名重複はprepare失敗。コアはMAC/FCS/FDBを解釈する比較規則を保持しない |
| FabricContext | 一実行につき一contextを所有し、device ID→endpoint/switch、output ID→Queue/Direction、MAC cache、frame cache、FrameMap/TransferMap/ReceptionMapとdispatcher cursorを持つ。Endpoint/Switch callbackはcontextへのIDを持つadapter。表はUTF-8順BTreeMap等で反復可能にし、hash iteration順は観測順へ使わない（注） |
| 結合順 | NED解決→型/port/schema照合→parameter値域→リンク逆方向対とtree検証→model-configの全instance/MAC/FDB照合→workload構文と全frame cache→generator cursor→initialize。欠落・重複を準備失敗。接続されたEndpoint2個のdirect linkも受理 |
| tree検証 | pair=(小さいdevicepath,大きいdevicepath)を一辺として作成。port対から同peer一組であること、Endpoint degree1、Switch degree≥2を確認。UnionFindで追加時同じrootならcycle、最後にcomponent1を確認。逆方向channelとbitrate/delayが一致しない辺はinvalid_connection |
| 不変frame | frame cacheは(source MAC,destination MAC,EtherType,data bytes)に対して計算したMAC bytes、payload length、pad、FCS。workload順の異なる同内容を共有可能。cache allocation失敗はprepare失敗 |
| dispatcher | 全cursorの次時刻から最小を一件だけ予約。同時刻のgenerator ID/ordinal順で生成してjournalへFrameを追加し、ready通知を予約。T以降の要素は論理次候補として保持し展開を遅延する。cursor加算はu128で検査、時刻は入力済u64。kind未知はunsupported_syntax |
| 初期化順 | 全simple adapterを安定ID順に初期化し、その後rootに対応する`@profile:ethernet.l2.store-forward.v1:<rootpath>` coordinatorを初期化する。adapterは構築済FabricContextの識別子だけを保持し、coordinatorが初期journal・dispatcher予約を確定する。全件成功後に時刻0を実行し、終了はcoordinator→simple逆順とする |
| 初期snapshot | 全queue空、方向idle/owner=null、Frame/Transfer/Reception台帳空、cursor0。metadataの初期stateは下表の正規形へ直列化する。finishで内部map/cacheを解放してもjournalは保持する |

| 初期stateのdata形 | 必須field |
| --- | --- |
| Endpoint | `{mac:S,queues:{tx:[]},links:{tx:{owner:null,state:"idle"}},generators:[{id:S,next_ordinal:"0",next_time_ps:D?}]}`。next_timeは先頭時刻、空列ならnull。generatorsはID順 |
| Switch | `{fdb:[{dst_mac:S,egress:S}],queues:{tx_suffix:[]},links:{tx_suffix:{owner:null,state:"idle"}}}`。全接続outputを列挙、FDBはMAC順 |
| 構造instance/Link | 構造instance=`{}`。Link=`{bitrate_bps:D,delay_ps:D}`。objectキーは再帰的UTF-8辞書順、metadata.stateはcompact JSON文字列 |

<a id="serialization"></a>

## 2. CRC・時刻・payload codec

```trace
{
  "id": "design-ethernet#serialization",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0146",
    "DIR-REQ-0147",
    "DIR-REQ-0149"
  ],
  "upstream": [
    "architecture#arch-ethernet"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 詳細設計 |
| --- | --- |
| 根拠仕様 | [wire](../specs/models/Ethernetモデル詳細機能仕様書.md#wire)のbyte順/CRC/preamble/IFG。serializerは関数encode_frame(source_mac,FrameInput)→FrameBytes又はPrepareError |
| 入力境界 | JSON重複を保持して検出できるreaderからstrict objectを構築。boolはintegerの代用として扱わず、MAC6byteとhex偶数桁を検査。data length0..1500。padを46まで00で充填し、EtherTypeをbig-endian2byteで追加する |
| CRC状態 | u32 registerをFFFFFFFFから開始、反転多項式EDB88320のLSBレジスタ更新をbyteごと8回。final xorFFFFFFFF。FCSをlittle-endian4byteでMAC bytesへappend。独立期待値はfixture固定値とPython zlib CRCおよび別bit実装で照合する |
| 時間関数 | duration(bits,R)=(bits×10^12+R−1)//Rをchecked u128で求めu64へ検査。EOF/releaseはSOFへchecked加算、arrivalはEOF+delay、処理完了はarrival+処理delay。将来予約もu64超過はE-0004 arithmetic_overflowで当該batchを失敗にする |
| codec | イベントschemaを`dir.ethernet.<kind>.v1`として下表の各kind別に登録し、各descriptorのphaseを固定する。Arrivalだけがport messageで、残りは所有modelへのTimer。byte codecは共用する。UTF-8 compact JSON・キー辞書順、数値ID/時刻はD。共通`{schema_version:1,kind:S,frame_id:S,transfer_id:S?,output:S?,reception_id:S?}`に下表の識別値を設定。byte payloadは同一FabricContext内の不変FrameMapの参照とする。将来profileで外部device連携を追加する際はbyte列を運ぶ別版codecを登録する。注：本profileのruntime内handleを外部ファイルへ直列化する方式は対象外 |
| 検証 | kindは受信schema名のkind部分と一致させ、kind別不要fieldはnull、必須識別fieldはS。参照が無い・所有output不一致・予定時刻とEnvelope時刻不一致・重複EOF/release・schema未知ならinvalid_event/model_failed。E-0002で全停止し未commit効果を公開しない |

| kind | 非null識別field | phase・対象 |
| --- | --- | --- |
| SourceReady | frame_id,output | 1、source Endpoint |
| Eof / Release / Arrival | frame_id,transfer_id,output | EofとReleaseは別schemaでphase0・送信owner。Arrivalは別schemaでphase1・受信deviceへsend_at。outputは送信元direction |
| ProcessComplete | frame_id,reception_id | 正delay完了0で同時刻phase1処理通知を作成 |
| ProcessReady | frame_id,reception_id | 1、reception owner。0delayの場合もこの通知を使用 |
| Generate / Arbitrate | runtime固有descriptor | dispatcher/dirty resourceの共通contractを使い、このpayloadへ重複の疑似timerを追加しない（注） |

<a id="forwarding"></a>

## 3. 状態遷移・原子性・終了

```trace
{
  "id": "design-ethernet#forwarding",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0148",
    "DIR-REQ-0150",
    "DIR-REQ-0151",
    "DIR-REQ-0152",
    "DIR-REQ-0153",
    "DIR-REQ-0156"
  ],
  "upstream": [
    "architecture#arch-ethernet"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 契機 | 前提 | 確定効果 |
| --- | --- | --- |
| Generate | workloadの未生成ordinal | frame行作成、SourceReady予約、cursor更新。TX処理は元frameごと独立 |
| SourceReady | frame.ready=null | ready時刻記録、parent=nullのtransferを一件offer。capacity0もdropped行を作る |
| offer | outputが存在しtransfer ID未登録 | queued又はdropped行、queue遷移、dirty output登録。offer順は現在のcallback順 |
| phase2 | direction idleかつqueue非空 | EOF/release/arrival時刻を事前検証し、先頭除去、transmitting/owner設定、予定時刻記録、EOFとReleaseをこの順でphase0予約。ArrivalはEOF成功後に予約 |
| Eof | ownerが対象でtransmitting、予定EOFと現在一致 | transfer serialized/eof設定、direction interframe、Arrivalを予定到達時刻phase1に予約。正常フレームが実送信完了したことをここで確定 |
| Release | owner一致でinterframe | release実績、direction idle/owner=null、dirty登録。到達待ちでも送信器を解放 |
| Arrival | serialized、arrival未設定、予定到達時刻一致 | transfer arrival実績、reception作成。endpoint不適合はfiltered、適合又はswitchはprocessingと予定処理完了を登録 |
| ProcessReady endpoint | processing、予定時刻一致 | receivedとready時刻を記録、delivery標本一件。元sourceへの自己配信はトポロジー上のcopyが到達した場合だけで、ローカル特例は作らない |
| ProcessReady switch | processing、予定時刻一致 | FDB候補を確定、same_ingressならfiltered。その他は全egressを順にofferし、forwardedと全transfer IDを同batchへ記録 |
| stop | T境界又はruntime失敗 | 凍結journalの未完了Frame/Transfer/Receptionを保持。失敗候補の予約はpendingに残し、それまでの成功committed prefixを出す |

| 内部規則 | 内容 |
| --- | --- |
| 状態所有 | Directionはqueueとowner、FrameMapは不変bytesとready、TransferMapは各時刻、ReceptionMapは判断結果。全directionを同一contextで扱い、fanoutが異なるmodelへの同期mutable呼出しを要求しない構造にする |
| batch検証 | 状態delta・全予約・全journal観測を組み立て、算術/容量割当/参照を確認してからcallback結果として渡す。核の効果検証成功でjournal公開。モデル内部が適用済でも核失敗時は全停止しjournalから復元。任意context全体clone・callback再実行は対象外（注） |
| 通常drop | queue_fullはcopyの正常な終端状態であり親receptionはforwarded。acceptedコピーとdroppedコピーを同じparentへ結び、forwardedを「全出力成功」の意味へ拡大しない |
| 物理複製の上限 | treeのsource根から各edgeを外向きに最大一回使う。parent chainは有限、同じtransfer ID再生成は不変条件違反として止める。FDB誤設定で入力方向を指す場合はsame_ingressへ終端する |
| phaseの確認 | 過去phaseへの同時刻予約は次delta。EOFとreleaseは正のIFGがあるため同時にならない。Arrival delay0はEOF phase0から同delta phase1へ。Switch正delay完了はphase0→phase1、0delayは同phase1、そこからphase2でSOF |

<a id="observations"></a>

## 4. 計測・保存則・検証割当

```trace
{
  "id": "design-ethernet#observations",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0155",
    "DIR-REQ-0156"
  ],
  "upstream": [
    "architecture#arch-ethernet"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 根拠・出力 | [outputs](../specs/models/Ethernetモデル詳細機能仕様書.md#outputs)の厳密data schemaを登録。ResultCollectorはschema_nameと正規JSON objectを保持し、Ethernet固有fieldへ分岐するコードを共通exporterへ置く処理は対象外（注） |
| journal更新 | row upsertはキー(schema_name,record_id)で一意。生成/offer/SOF/EOF/release/arrival/処理完了の成功batchごとに更新。row.time_psは更新callback時刻。予定時刻は実績nullを上書きする根拠として使わない |
| 分子 | 結果側のbusy積分はdirection状態とSOF/release実績で作り、Hでclip。伝搬時間はbusyへ加えない。EOF量とrelease量の点は独立に加算し、元frameとhop複製の二重/過少計数を区別する |
| 終端照合 | offeredの4状態保存則、arrived=reception数、source ready数=parent null copy数、各forwarded子ID集合=parent一致copy集合。bytes/FCSは全hopでFrameMapと同じ |
| 検証割当 | [DIR-TEST-0023～0028](../verification/cases/Ethernetモデル検証仕様書.md)へserialization、duplex、switch/fanout、queue、prepare、stopを分担。共通runtime/outputの検証は既存ケースを組み合わせる |
