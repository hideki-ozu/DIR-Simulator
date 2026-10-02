# メモリ・IPC詳細機能仕様書

文書バージョン：`0.1.1`
対象GitHubバージョン：`v0.1`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。メモリ・IPCの仕様・設計・解析fixtureを定義 |

文書ID：`spec-memory-ipc`

| 項目 | 契約 |
| --- | --- |
| 状態 | 仕様・設計・解析fixtureを規定。製品実装・製品実行試験は未実施。全時間・仲裁・公開規則は本プロジェクトが選択する抽象policyであり、特定DDR世代・JEDEC・CPU命令セットへの適合を表明するものではない（注） |
| profile | `memory.ipc.transaction.v1`。INIのmodel-profileとmodel-configを必須とし、[共通仕様](../拡張モデル共通詳細機能仕様書.md)のschema2 workload/results、入力snapshot、u64 psとjournalを共用する |
| 型記法 | D=先頭0なし非負十進文字列（0可、u64範囲）、P=正のD、N=非負JSON整数token（boolean/指数/小数不可）、S=文字列、?=null可。hexは小文字・偶数桁。全階層で明記したキーを必須とし未知・重複・型違反を準備失敗とする |
| root | `{schema_version:1,profile:"memory.ipc.transaction.v1",ddr:Ddr[],sram:Sram[],shared:Shared[],dma:Dma[],mailboxes:Mailbox[]}`。各配列は空可、合計一件以上。nodeはNED完全パスで全体一意。配置された本profile資源を全件ちょうど一度含む |
| Registry | `dir.memory.DdrV1/SramV1`、`dir.ipc.SharedV1/MailboxV1`、`dir.dma.EngineV1`を固定implementationキーとして登録。全てNED parameterとgate集合は空。モデル構成はJSONで与え、coordinatorがprepare済みhandleへだけ内部transactionを発行する。protocol=`dir.memory-ipc.transaction`、messageは各内部Kind、schema=`dir.memory-ipc.transaction.<Kind>`版1。各schemaを固定phaseへ個別登録する |
| workload | root=`{schema_version:2,generators:[...]}`。generator=`{id:S,kind:"memory-ipc.explicit.v1",node:S,times:S[],request:Request}`、timesはpsへ正確換算可能な非減少時間文字列。ID/同時生成順は共通仕様。Requestはtargetの種類別に下記の完全キー集合を使用。request_id=`id:ordinal`、[0,T)内の発火だけをofferする |
| 順序・queue | phase1 Offerは要求をawaiting_admissionとしてcoordinatorの(time,delta)別intent集合へ収集し、offered件数と要求行をcommitする。phase2の共通dirty coordinatorは同集合の全要求を先にadmissionし、待機FIFO capacity未満ならqueued、満杯ならrejected/queue_full。その後で各資源をdispatchする。範囲・権限の通常拒否も同じadmissionで容量検査より先に確定する。同じ(time,delta)の全offerを`(root_generator_id UTF-8順,root_ordinal 数値順,source_rank,chunk_index,operation_rank)`昇順でphase2 admissionする。外部要求はsource_rank=0/chunk_index=0/operation_rank=0、DMA childは親generator/ordinalを継承しsource_rank=1、chunk_indexは0始まり、read=0/write=1。外部とchildを別々の順序列にしない（注）。動的child/responseも共通Contextの規則で予約し、phase0/1→phase1は同じdelta、phase2→phase1だけ次deltaとなる。同deltaの再帰的なphase1 callbackが全て終わってからphase2が走るため、その集合の全offerをadmission前に収集できる。activeは待機容量外、capacity=0は全offerを拒否。phase0完了→phase1 intent収集→phase2全admission→phase2先頭dispatch。awaiting_admissionは有限FIFOの待機長へ含めず、失敗prefixではその状態と入力fieldを要求行へ残す。容量を空きportの先取りで回避する処理は対象外（注） |
| 時刻と停止 | 全latency/refresh/notificationは正ps、容量は下記範囲。checked u128で式を評価し、予約時刻をu64へ検査。E-0004はそのcallback効果をcommit前に破棄。Tのeventは実行せずqueued/active/publishing/notifying等を保存。失敗時も成功journal prefixのbyte内容と件数だけを公開する |
| 結果エラー | 未知node/op・schema・入力長違反・未配置参照はprepare E-0001。実行時address_error、queue_full、access_denied、empty、full、memory_faultは通常responseで実行継続。message型違反はE-0002 invalid_event、owner/世代不整合はmodel_failed |
| 組合せ | 本profile内で複数DDR/SRAM、共有slot、mailbox、DMAを配置可能。DMAだけがDDR/SRAMの同一サービスを参照する。shared/mailbox内容はそれぞれの所有領域内で完結する。注：AXI接続、cache coherence、CPU命令実行、共有slotをDMAが直接更新する経路、他profileとの混成は本版の対象外。将来の複合profileは新しい版付きadapterで追加する |

## 構成・動作・結果をつなぐ確認単位

本profileの五つの親要件は1.0.0以後の将来対象である。それぞれの構成子要件が容量・初期内容・時間定数を確定し、動作子要件がadmission・dispatch・内容公開を定め、結果子要件がbyte内容・応答・停止prefixの対応を確認する。以下は各節の契約をつなぐ読み方である。

| 親要件 | 準備から資源利用まで | 内容・終了結果の確認 |
| --- | --- | --- |
| [DDRメモリ](../../要件定義書.md#dir-req-0198) | [ddr](#ddr)の形状・初期byte・行境界を検証し、単一FIFO serviceでopen rowとrefreshによる待ちを適用する | completionで公開/採取したbyteとrow/refresh状態を[DIR-AC-0048](../../要件定義書.md#dir-ac-0048)で照合する |
| [SRAMメモリ](../../要件定義書.md#dir-req-0202) | [sram](#sram)の範囲・port数を確定し、FIFO先頭から空portを割り当てる | 同時completionのdispatch ordinal順で決まる内容とport占有を[DIR-AC-0049](../../要件定義書.md#dir-ac-0049)で照合する |
| [共有メモリIPC](../../要件定義書.md#dir-req-0206) | [shared](#shared)のactor権限・slotを確定し、publish/consumeの所有権を進める | 公開前、ready、consume中の内容とownerを[DIR-AC-0050](../../要件定義書.md#dir-ac-0050)で照合する |
| [DMA](../../要件定義書.md#dir-req-0210) | [dma](#dma)の両端範囲を検証し、chunkごとのread/write childを通常DDR/SRAM資源へofferする | write済みcommitted_bytes、親子要求、data_doneと通知完了の差を[DIR-AC-0051](../../要件定義書.md#dir-ac-0051)で照合する |
| [メールボックスIPC](../../要件定義書.md#dir-req-0214) | [mailbox](#mailbox)のactor、操作待機容量、payload FIFO容量を確定し、send/receiveをservice順に評価する | enqueue/dequeueと遅延notificationを別に追い、payload残数と通知状態を[DIR-AC-0052](../../要件定義書.md#dir-ac-0052)で照合する |

共通phase順とjournalは状態確定の土台であり、どの時点で内容が可視になるかはモデル固有である。DDR/SRAMはcompletion、sharedはpublish completion、mailboxはsend completion、DMAの進捗は各write childのcommitで決まる。DMAの成功終端はその後の通知、mailbox sendの成功終端はenqueue時であり、同じ通知待ちとして親要求の状態を統一しない。

Tで打ち切る確認では未完了writeの予定byteを現メモリへ補わず、公開済みslot、FIFO内payload、DMAの部分進捗と未通知行を[records](#records)の完全schemaへ対応付ける。親要件の判断は[詳細設計](../../design/メモリ・IPC詳細設計書.md)の所有状態と[検証仕様](../../verification/cases/メモリ・IPC検証仕様書.md)の独立期待値を合わせ、通常responseの拒否と実行失敗を区別して行う。

<a id="ddr"></a>

## 1. DDRメモリ

```trace
{
  "id": "spec-memory-ipc#ddr",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0198",
    "DIR-REQ-0199",
    "DIR-REQ-0200",
    "DIR-REQ-0201"
  ],
  "upstream": [
    "DIR-FUNC-0047"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| Ddr | `{node:S,size:N,initial:Init[],queue_capacity:N,banks:N,row_bytes:N,rows_per_bank:N,width_bytes:N,open_ps:P,close_ps:P,column_ps:P,beat_ps:P,refresh_interval_ps:P,refresh_ps:P,fault_ranges:Range[]}`。size=1..65536、banks=1..16、row_bytes/rows_per_bank/width_bytes=1..65536、size=banks*row_bytes*rows_per_bank、row_bytesはwidth_bytesの倍数、queue_capacity=0..1024、refresh_ps<refresh_interval_ps |
| byte初期化 | Init=`{offset:N,hex:S}`、非空hexをsize内の非重複範囲へ配置し残りbyte=00。Range=`{offset:N,length:N}`、length>0、size内・非重複。fault範囲へ重なる要求はdispatch時にmemory_faultとなり、内容を変えず資源を占有しない |
| Request | read=`{op:"read",address:N,length:N}`、write=`{op:"write",address:N,length:N,hex:S}`。length=1..65536、writeはlength byte一致。address/lengthはu64値域。size外、終端加算超過又は一行を跨ぐ要求はadmissionでrejected/address_error。bank=(address div row_bytes) mod banks、row=address div (row_bytes*banks)、column=address mod row_bytes |
| 仲裁・時間 | 全bank共通の単一service資源でFIFO。初期open_row[bank]=null。grant=gで閉行ならsetup=open_ps、同一行hitなら0、別行ならclose_ps+open_ps。completion=g+setup+column_ps+ceil(length/width_bytes)*beat_ps。grant時に対象bankのopen_rowをrowへ変更。他bankのrowは保持。注：bank並列実行・JEDEC command/timing網羅は対象外 |
| 内容 | write全byteはcompletion phase0で一括可視化、read値はcompletion直前のcommit済みメモリから採取。直列サービスのため通常read/writeはFIFO可視化順。同時刻refreshの開始は内容を変えない |
| refresh | 初回due=refresh_interval_ps。due到達phase0でrefresh_pending=trueとして新dispatchを抑止し、active完了後のmax(due,completion)でrefresh開始。idleならdue直ちに開始。全open_row=null、refresh_end=start+refresh_ps、次due=start+refresh_interval_ps。終了までdispatch待機。dueとcompletion同時刻はrefreshを先に新dispatchより優先。待機queueを保持する。実際のrefresh_start/endを行へ記録する |
| 観測 | ddr.read/write bytes、hit/miss、refresh count、service/refresh区間、応答時刻を記録。Tがcompletionならwrite内容は以前のまま、予定row/予定完了と実績を別欄へ保存する |

<a id="sram"></a>

## 2. SRAMメモリ

```trace
{
  "id": "spec-memory-ipc#sram",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0202",
    "DIR-REQ-0203",
    "DIR-REQ-0204",
    "DIR-REQ-0205"
  ],
  "upstream": [
    "DIR-FUNC-0048"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| Sram | `{node:S,size:N,initial:Init[],queue_capacity:N,ports:N,read_ps:P,write_ps:P,fault_ranges:Range[]}`。size/初期/faultはDDRと共通、ports=1..16、queue_capacity=0..1024。全byte範囲はsize内、行境界制約を持たない |
| Request | read/writeのキー集合はDDRと同じ。範囲外はadmission時address_error。fault範囲重複はdispatch時memory_fault。memory_faultでportを占有しない |
| port割当 | phase2で待機先頭へ空portの最小indexを割当し、空port又は待機が尽きるまで反復。read完了=g+read_ps、write完了=g+write_ps。重複アドレスも並行受理する |
| 可視性 | completion時にread採取/write全byte公開。同じ資源・同じ完了時刻はgrant時の単調dispatch ordinal昇順でphase0 batchを処理し、先行writeは後続readから可視。同時write重複は後続ordinalのbyteが最終値。別時刻は完了時刻順。これは選択した決定的抽象policyである |
| 停止・測定 | Tでactive writeを補完せず、commit済みbyte列・port所有者・待機列を残す。port別busy積分と全port平均利用率を分離し、読書き件数/byte数/queue待ちを保存する |

<a id="shared"></a>

## 3. 共有メモリIPC

```trace
{
  "id": "spec-memory-ipc#shared",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0206",
    "DIR-REQ-0207",
    "DIR-REQ-0208",
    "DIR-REQ-0209"
  ],
  "upstream": [
    "DIR-FUNC-0049"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| Shared | `{node:S,slots:N,slot_bytes:N,queue_capacity:N,publish_ps:P,consume_ps:P,producers:S[],consumers:S[]}`。slots=1..1024、slot_bytes=1..65536、slots*slot_bytes<=65536、queue_capacity=0..1024。actorはASCII識別子、各配列非空・重複なし、両配列の重複可 |
| Request | publish=`{op:"publish",actor:S,hex:S}`、consume=`{op:"consume",actor:S}`。publish hexは1..slot_bytes byte。actorが該当producer/consumer集合にない場合admissionでrejected/access_denied |
| 所有権 | 各slotはfree→publishing→ready→consuming→free。phase2で単一操作serverが待機先頭をdispatch。publishは最小index free slotをproducer actorが占有、予定completion=g+publish_ps。freeなしならrejected/fullとして即時終了し次FIFOへ進む。consumeはready FIFO先頭をconsumer actorが占有、予定completion=g+consume_ps。readyなしならrejected/empty |
| 公開・取得 | publish completionでlength/hexをslotへ一括commitしてready FIFO末尾へ追加しownerをnullとする。consume completionで同じhexをread結果へcopyしslot free、length0/hex空/owner nullへ戻す。publishingのpayloadはslot未公開で要求側だけに保持。consumingは公開済みbytesを保持するが別consumerの取得対象から外す |
| 順序と停止 | 単一serverなのでpublishとconsumeはFIFO service順。同時刻publish completion phase0の後にconsumeをdispatchできる。T前に公開した未取得内容とT途中のproducer/consumer ownerを残す。注：CPU cache/fence命令を実装するのではなく公開/取得という抽象境界を定義する |
| 計測 | published/consumed/拒否理由別件数、ready件数、占有slot数、未公開/公開/消費中payloadを区別。published=consumed+ready+consuming |

<a id="dma"></a>

## 4. DMA

```trace
{
  "id": "spec-memory-ipc#dma",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0210",
    "DIR-REQ-0211",
    "DIR-REQ-0212",
    "DIR-REQ-0213"
  ],
  "upstream": [
    "DIR-FUNC-0050"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| Dma | `{node:S,queue_capacity:N,chunk_bytes:N,setup_ps:P,notify_ps:P}`。queue_capacity=0..1024、chunk_bytes=1..65536。一engineでactive descriptor一つ |
| Request | `{op:"copy",src:S,dst:S,src_address:N,dst_address:N,length:N}`。src/dstは同profile内DDR/SRAM node、length=1..65536、addressはu64。両端rangeをadmissionで検査し違反はaddress_error。同一memoryの重複範囲はrejected/address_error。非重複同一memoryは受理する |
| chunk分割 | offset0からcount=min(chunk_bytes,残長,src DDR行残byte,dst DDR行残byte)として増分。SRAM端は残長を使う。setup終了g+setup_psに最初のread childをphase1 offerし、各read response後に同時刻phase1でwrite childをoffer。write response後に次chunk readを同様にofferする。deltaは共通Contextの予約規則に従う |
| 共有資源 | childは通常DDR/SRAM FIFO・容量・fault・refresh・port policyを使う。read completionのbyte snapshotをwrite payloadへ固定する。直接trafficも同じmemory内容と資源を使用する。read/write子ID=`parent/r/index`と`parent/w/index`、origin_request_id=親ID。注：固定memcpy時間でサービスを飛ばす処理は対象外 |
| 完了と失敗 | write child completed/okごとにcommitted_bytesをcount増分。最終writeでdata_done_psを記録しnotifying、notify予定=data_done+notify_ps。通知実行phase1でcompleted/ok、completed_ps=notify時刻、engine解放。child拒否又はmemory_faultなら最初の失敗応答を処理するDmaResponse時刻をcompleted_psに保存しfailed、reason=child_<reason>、data_done=null、通知を作らず解放。以前のwrite内容とcommitted_bytesを保持する |
| 停止 | T途中はawaiting_admission/queued/setup/reading/writing/notifyingを保存し、未実行通知・未完了writeを実績化しない。committed_bytesはwriteの実commitのみ。parentとchildの同callback journalでbyte内容/進捗更新を原子的に保存し、失敗prefixで二重計上しない |
| 観測 | descriptor件数、committed_bytes、最終通知までのend-to-end遅延、child記録と親対応を出す。active descriptorが異なるengine間ではmemory FIFO規則で競合する |

<a id="mailbox"></a>

## 5. メールボックスIPC

```trace
{
  "id": "spec-memory-ipc#mailbox",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0214",
    "DIR-REQ-0215",
    "DIR-REQ-0216",
    "DIR-REQ-0217"
  ],
  "upstream": [
    "DIR-FUNC-0051"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| Mailbox | `{node:S,capacity:N,payload_bytes:N,queue_capacity:N,service_ps:P,notify_ps:P,senders:S[],receivers:S[]}`。capacity/queue_capacity=0..1024、payload_bytes=1..65536、capacity*payload_bytes<=65536、actor集合はsharedと同じ |
| Request | send=`{op:"send",actor:S,hex:S}`、receive=`{op:"receive",actor:S}`。send hexは1..payload_bytes byte。権限不一致はadmissionでrejected/access_denied。descriptor待機容量とpayload FIFO容量は別資源 |
| service | 単一serverがFIFO先頭をgでdispatchし、各opをg+service_psで評価。send時点でpayload queue長<capacityならmessage_id=request_idを末尾へ追加、enqueued_ps=completion。満杯ならcompleted/full。receive時点で非空なら先頭を取り出しcompleted/ok、hex/message_idを保存。空ならcompleted/empty。通常responseもserverを解放する |
| notification | 成功sendごとにnotification行を生成しplanned_ps=enqueued_ps+notify_ps、delivered_ps=null。phase1通知時にdelivered_psと宛先receiversの全actorを記録する（一つのbroadcast観測、request自体はsend completionで完了）。receive済みでもそのmessageの到着通知は予定通り一度送る。notificationは受信の前提条件ではなく観測可能な到着event |
| 順序・停止 | payload FIFOは成功enqueue順。Tで未通知ならplannedのみ、すでにreceive済みならqueueから除去した事実と未通知行をともに保存する。注：CPU interrupt優先度・割込mask・OS schedulerは対象外 |
| 計測 | sent/received/full/empty、queue length/time mean、notify deliveredとenqueue→notification遅延。sent=received+payload queue長、notify delivered<=sent |

<a id="records"></a>

## 6. 完全な結果・内部codec・metric登録

| 項目 | 契約 |
| --- | --- |
| ModelRecord | 共通schema2包絡のsubjectはnode、record_idは以下、request_idは要求行=要求ID、資源/通知行=null、origin_request_idはDMA childだけ親ID。time_psは最後のcommit時刻。全field必須。下記全schemaを版1でmetadataへ登録 |
| memory-ipc.request/1 | record_id=要求ID。data=`{model:S,op:S,actor:S?,status:S,reason:S?,generated_ps:D,started_ps:D?,planned_completion_ps:D?,completed_ps:D?,address:D?,length:D?,input_hex:S?,output_hex:S?,slot:N?,port:N?,dispatch_ordinal:D?,bank:N?,row:N?,row_hit:boolean?,message_id:S?,src:S?,dst:S?,src_address:D?,dst_address:D?,committed_bytes:D,data_done_ps:D?,planned_notify_ps:D?,notified_ps:D?}`。対象外field=null、committed_bytesはDMAだけ進捗でその他0。status=awaiting_admission/queued/active/completed/rejected又はDMA setup/reading/writing/notifying/failed。成功reason=ok、pending reason=null。rejectedはcompleted_ps=拒否時刻、started=null（dispatch時拒否はstarted=dispatch時刻）、planned=null |
| 要求行の型別値 | memoryはmodel=ddr/sram、address/length/input又はoutput、DDR bank/row/hit、SRAM portを設定。sharedはmodel=shared、actor/input又はoutput/slot、mailboxはmodel=mailbox、actor/input又はoutput/message_id。DMAはmodel=dma、src/dst/addresses/lengthとcommitted/通知欄。awaiting_admission時にinput fieldは確定し、dispatch固有fieldはnull。read/consume/receiveのoutputは成功completionだけに設定する |
| memory-ipc.memory/1 | record_id=node。data=`{kind:S,size:D,hex:S,open_rows:(N?)[],ports:(S?)[],refresh_pending:boolean,refresh_started_ps:D?,refresh_planned_end_ps:D?,refresh_ended_ps:D?,queue:S[]}`。hexはsize byte完全像、DDR portsは単一要求ID又はnull、open_rowsはbanks個。SRAM open_rows空、portsはports個、refresh関連false/null。refresh start/endは直近実績で、新refresh開始時ended=null |
| memory-ipc.shared/1 | record_id=node。data=`{slots:[{index:N,state:S,owner:S?,message_id:S?,hex:S}],ready:N[],active:S?,queue:S[]}`。slot message_idはpublish要求ID、freeはnull/hex空、publishingはmessage_idのみ確定・hex空、ready/consumingは公開hex |
| memory-ipc.mailbox/1 | record_id=node。data=`{messages:[{message_id:S,hex:S,enqueued_ps:D}],active:S?,queue:S[]}`。messagesはFIFO順 |
| memory-ipc.dma/1 | record_id=node。data=`{active:S?,queue:S[],child:S?,chunk_index:D,chunk_hex:S?}`。idle時child/hex=null、index0。writing中chunk_hexはsource read snapshot |
| memory-ipc.notification/1 | record_id=成功send ID。data=`{message_id:S,receivers:S[],enqueued_ps:D,planned_ps:D,delivered_ps:D?}`。宛先はUTF-8順 |
| codec | 各kindのbodyは共通の`{kind:S,node:S,request_id:S?,generation:D}`。kind=Offer/Complete/RefreshDue/RefreshEnd/DmaSetup/DmaResponse/DmaNotify/MailNotifyで、protocol=`dir.memory-ipc.transaction`、message=kind、schema=`dir.memory-ipc.transaction.<Kind>`版1をそれぞれ登録。schemaとbody.kindの一致を必須とする。全field必須、compact UTF-8 JSON辞書順。payload本体は不変PreparedRequests/ChildRequestsをID参照し、node/owner/state/世代を検証する。Refreshはrequest_id=null、MailNotifyはsend ID。Offer1、Complete/Refresh/DmaSetup0、DmaResponse/DmaNotify/MailNotify1。generationは予約ごとchecked増分し資源token台帳に保持 |
| 共通metric集合 | 以下だけをmetadataへ版1登録する。全metricの対象は各資源node、$all集計を暗黙追加しない。補助列receiver/reason/sample_countはnull、pointのrequest_idは原因要求ID（初期null）、summaryはnull。H/窓/空集合/partialは共通結果仕様を適用する |


| metric ID | unit / value_kind / sampling / aggregation | 値 |
| --- | --- | --- |
| `memory_ipc.offered`, `memory_ipc.completed`, `memory_ipc.rejected` | count / integer / summary / sum | 外部・child要求を資源ごとに計数。completedは通常エラー応答を含む、failed DMAはrejectedに含む。offered=awaiting_admission+queued+active各状態+completed+rejected |
| `memory_ipc.committed_bytes` | byte / integer / summary / sum | DDR/SRAMは成功write byte、DMAは親committed_bytes、sharedはpublish byte、mailboxはsend byte。異なる資源のbyteを同一転送量として加算する処理は対象外（注） |
| `memory_ipc.queue_length` | count / integer / point / identity | 初期0とphase2 admission/dispatch/不変reject時の待機FIFO長。phase1収集だけでは変化しない |
| `memory_ipc.queue_mean` | count / number / summary / time_mean | 待機長の[0,H)積分/H、H0はnull |
| `memory_ipc.busy_ratio` | 1 / number / summary / occupancy_ratio | DDR service区間、SRAM各port占有和/(ports*H)、shared/mailbox service区間、DMA active全区間。H0はnull、正Hでidleは0 |
| `memory_ipc.latency_ps` | ps / integer / point / identity | completed/rejected/failed要求のcompleted_ps-generated_ps。DMA成功は通知まで |
| `memory_ipc.ddr_refreshes` | count / integer / summary / sum | DDRだけ、開始したrefresh数。完了数と区別する |
| `memory_ipc.notifications` | count / integer / summary / sum | DMA/mailboxだけ、実通知数。予定通知は数えない |

| 補足 | 契約 |
| --- | --- |
| refresh計測 | serviceのbusy_ratioとrefreshを区別し、refresh実時間はmemory行と内部journalで保持する。metric提供範囲は上表のkind条件で確定する。注：busy_ratioへrefresh区間を含めない |
