# メモリ・IPC詳細設計書

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。メモリ・IPCの仕様・設計・解析fixtureを定義 |

文書ID：`design-memory-ipc`

| 項目 | 契約 |
| --- | --- |
| 構造 | `MemoryIpcProfile`はprepareでNodeHandle、immutable request、geometry、actor集合とFIFO容量を確定する。`Coordinator`はDDR/SRAM stateとIPC/DMA stateを所有し共通Context/journalへ差分を提出する。製品実装は未実施 |
| 準備 | 配置を正規path順に解決し型とconfig集合を一対一照合。全workloadをT以後も検証。node/op、geometry、初期範囲、actor、DMA参照の診断をsource順で確定し、成功後だけ初期memory/slot/mailbox/engine行を一括commitする |
| dispatch | 共通runtimeのphase0/1/2を使う。同資源の同時刻completionを一つのbatchとしてordinal順に処理。phase1 Offerはawaiting_admission行とintentをcommitする。phase2 dirty coordinatorは全intentを仕様の(root generator,ordinal,source rank,chunk,operation)で全順序化してadmissionし、その後に資源path順でdispatchする。callbackが予約するphase1はphase0/1起点なら同delta、phase2起点なら次deltaであり、共通Context規則をそのまま使う。注：他資源の関数を再帰実行する方式は対象外 |
| 安全な効果 | callback内でchecked時刻とcodecを検証→state差分/次timer/metric/ModelRecordを構築→journal commit。失敗時はbatch以前の公開snapshotを使用し停止。予約とowner変更の片方だけを公開する処理は対象外（注） |
| 所有権 | coordinatorの資源handle tableにあるnodeだけがchild宛先になる。通常入力に任意codecやgenerationを渡すインタフェースを作る処理は対象外（注）。intent内・queue内・port内・DMA child IDは要求台帳へ一意参照する。intentはawaiting_admission行から再構成でき、FIFO queue長へ算入しない。phase1後の失敗prefixもoffered保存則へawaiting_admissionを含める |

<a id="ddr"></a>

## 1. DDRメモリ

```trace
{
  "id": "design-memory-ipc#ddr",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0198",
    "DIR-REQ-0199",
    "DIR-REQ-0200",
    "DIR-REQ-0201"
  ],
  "upstream": [
    "architecture#arch-memory-ipc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| State | ByteStore(size)、open_rows、pending FIFO、active(request,ordinal)、refresh_due/token/pending/active、last refresh timestamps。単一port所有者はactiveと一致する |
| 遷移 | phase1収集→phase2 admissionで範囲確認→FIFO容量確認、Arbitrateでfaultを拒否し正常先頭を開始。row種別とsetupを先に記録しComplete tokenを予約する。Completeはbyte差分又はread結果・port解放を同batch commit。RefreshDueでdirty、activeならpendingだけを立て、完了batchがrefreshを開始する |
| 境界 | dueとCompleteが同時刻なら全phase0効果を集めた後にrefreshを開始する。refresh中に新refresh dueは発生しない（interval>refresh）。RefreshEndでrefresh_activeを解きphase2 dispatchを許可する。終了時にtokenを解放して将来refreshを実績化しない |

| 正本 | 参照 |
| --- | --- |
| 仕様 | [対応契約](../specs/models/メモリ・IPC詳細機能仕様書.md#ddr) |

<a id="sram"></a>

## 2. SRAMメモリ

```trace
{
  "id": "design-memory-ipc#sram",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0202",
    "DIR-REQ-0203",
    "DIR-REQ-0204",
    "DIR-REQ-0205"
  ],
  "upstream": [
    "architecture#arch-memory-ipc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| State | ByteStore、Port[ports]、pending FIFO、next_dispatch_ordinal。各Portはownerと予定完了時刻、空きはnull |
| 遷移 | phase2でport index順に先頭を割当。CompleteBatchは同時刻に到達したownerをdispatch ordinal順で処理する。一時byte snapshotにwrite/read差分を順次適用し一回commitするため同時刻可視順が登録順から独立する |
| 検査 | 全queue+owner IDの重複なし、size一定、pending<=capacity、port数一定。同時write重複の最終byteと同時read結果をfixtureで照合する |

| 正本 | 参照 |
| --- | --- |
| 仕様 | [対応契約](../specs/models/メモリ・IPC詳細機能仕様書.md#sram) |

<a id="shared"></a>

## 3. 共有メモリIPC

```trace
{
  "id": "design-memory-ipc#shared",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0206",
    "DIR-REQ-0207",
    "DIR-REQ-0208",
    "DIR-REQ-0209"
  ],
  "upstream": [
    "architecture#arch-memory-ipc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| State | Slot[state,owner,message_id,bytes]、ready FIFO、active operation、pending FIFO。freeはbytes空、publishingは要求のinput bytesだけがpayloadを持つ |
| 遷移 | publish開始でfree最小slotをreserve、完了でbytesを公開しreadyへ移す。consume開始でreadyから除去しownerをconsumerへ、完了でoutput snapshotとfreeを同commitにする。full/emptyはdispatch時の即時通常拒否でserverを占有せず次候補へ進む |
| 検査 | ready FIFO内IDはready slotと完全一致、publishing/consumingはactiveに一対一。actorをprepareで構文検査しadmissionで権限結果へ分類する |

| 正本 | 参照 |
| --- | --- |
| 仕様 | [対応契約](../specs/models/メモリ・IPC詳細機能仕様書.md#shared) |

<a id="dma"></a>

## 4. DMA

```trace
{
  "id": "design-memory-ipc#dma",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0210",
    "DIR-REQ-0211",
    "DIR-REQ-0212",
    "DIR-REQ-0213"
  ],
  "upstream": [
    "architecture#arch-memory-ipc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| State | engine FIFO、active parent、chunk index/offset/length、source snapshot、child ID、setup/notify token。ChildRequestsは親と分割定義から生成するimmutable行 |
| 連携 | childをmemory Offer dispatcherへ登録する。memory完了commitの一部として親committed_bytesとwrite進捗を更新し、共通規則で同時刻phase1 DmaResponseを予約する。readは同commitで親chunk_hexを保存する。memory fault/reject時は親失敗処理を共通規則のphase1 DmaResponseで確定する |
| 二重処理防止 | child台帳にresponse_consumedフラグを持ち、同じ通知再処理はmodel_failed。write byteとparent committed更新は同journalであるためT境界でも一致。DmaResponseは次child又はdata_done/notifyingを選ぶ。data_doneは最終write実completionの時刻、同時刻の全phase/deltaをT未満なら実行する |
| 失敗 | child queue_full/address_error/memory_faultは親failedへ伝搬し次childを作らない。親failedのcompleted_psはDmaResponse時刻、既commit writeは保持。実行基盤失敗は通常failedに変換せず全run partialを保存する |

| 正本 | 参照 |
| --- | --- |
| 仕様 | [対応契約](../specs/models/メモリ・IPC詳細機能仕様書.md#dma) |

<a id="mailbox"></a>

## 5. メールボックスIPC

```trace
{
  "id": "design-memory-ipc#mailbox",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0214",
    "DIR-REQ-0215",
    "DIR-REQ-0216",
    "DIR-REQ-0217"
  ],
  "upstream": [
    "architecture#arch-memory-ipc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| State | Message FIFO(message_id,bytes,enqueued)、pending operations、active、NotificationMap。送信済みmessageが受信で除去されてもNotificationMapは保持する |
| 遷移 | service Completeでsend/receiveを評価しFIFOと要求行を同commit。成功sendはNotificationMapとMailNotify tokenを作る。MailNotifyはentry.delivered=nullを検証して時刻と通知metricをcommitし、queueは参照しない |
| 検査 | sent=received+FIFO長、capacityを超えない、message ID一意、全Notificationが成功sendを参照。送信拒否では通知entryを作らない |

| 正本 | 参照 |
| --- | --- |
| 仕様 | [対応契約](../specs/models/メモリ・IPC詳細機能仕様書.md#mailbox) |

## 6. 保存・検証・拡張

| 項目 | 契約 |
| --- | --- |
| schema | [結果とcodec](../specs/models/メモリ・IPC詳細機能仕様書.md#records)の全fieldを型付きserializerで保存。入力配列は正規化し資源path順、slot index順、message/FIFOは意味順を保つ |
| 停止 | 正常Tでは観測区間をH=Tでclipし状態を保持、finishは参照解放だけ。失敗では成功prefixのHを共通runtime規則で決定し予定完了・通知を実績へ変換しない |
| 拡張 | resource service境界はOfferと一つのResponse、内容はbyte列、所有者はNodeHandle。新profileでAXI adapterやcache policyを追加するとき既存profile/codec/順序を保持する |
| 検証 | [検証仕様](../verification/cases/メモリ・IPC検証仕様書.md)は製品未実装を明示し、独立整数解析・固定期待byte列と将来のjournal注入試験を区別する |
