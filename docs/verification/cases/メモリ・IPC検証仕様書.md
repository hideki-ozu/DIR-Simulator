# メモリ・IPC検証仕様書

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。メモリ・IPCの仕様・設計・解析fixtureを定義 |

文書ID：`verification-memory-ipc`

| 項目 | 契約 |
| --- | --- |
| 状態 | 検証仕様を規定。独立解析checkerは13ケースの固定期待値と6構成境界を照合する。製品シミュレータの実行・内部journal障害注入は未実施 |
| 入力 | [scenarios.json](../fixtures/memory-ipc/scenarios.json)が各NED/INI/model-config/workloadと期待projectionを列挙。[Types.ned](../fixtures/memory-ipc/models/memoryipc/Types.ned)、[Main.ned](../fixtures/memory-ipc/models/memoryipc/Main.ned)を共用する |
| 解析実行 | `python3 docs/verification/fixtures/memory-ipc/verify_expectations.py`。固定入力への整数式・byte配列演算で期待値を復算する。これはevent schedulerを持たない独立解析器であり製品実装の代替ではない（注） |
| 製品実行手順 | 実装後に`dir-simulator run --config docs/verification/fixtures/memory-ipc/ddr-row-refresh.ini --output /tmp/dir-memory-ddr`を実行し、残り各INIも未存在outputへ実行する。results.jsonのmemory-ipc.request/memory/shared/dma/mailbox/notification行をprojectionへ対応付け完全一致を判定する |
| 判定 | ps・byte hex・ID・状態・理由・件数は完全一致。比率/平均は共通binary64規則。停止と失敗でplannedとactualを分け、ModelRecordの全必須field、metadata schema/metric集合、source hashを製品検証で照合する |
| 解析の限界 | checkerは全schema validatorや状態機械を実装するものではない（注）。unknown key、型全域、動的競合、rollback、製品API・性能は以下の製品試験を必要とする。traceのconfirmedは検証仕様の確定であり製品合格を意味しない |

<a id="dir-test-0050"></a>

## 1. DDR行状態・refresh

```trace
{
  "id": "DIR-TEST-0050",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0198",
    "DIR-REQ-0199",
    "DIR-REQ-0200",
    "DIR-REQ-0201"
  ],
  "upstream": [
    "design-memory-ipc#ddr"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#ddr)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#ddr) |
| 上流 | [DIR-FUNC-0047](../../機能仕様書.md#dir-func-0047)、[DIR-AC-0048](../../要件定義書.md#dir-ac-0048) |
| 入力 | ddr-row-refreshのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 入力 | 2bank、4byte行、width2byte、open3/close2/column2/beat1ps、refresh interval15/duration5ps。write0/read0/write8/read0の生成0/7/11/16ps |
| 期待 | grant=0/7/11/25ps、completion=7/11/20/32ps、hit=false/true/false/false。due15をactive終了20まで待ちrefresh20..25。readはいずれもaabbccdd、commit8byte、service積分27ps |
| 計測・構成 | bank0の行変更、refreshで両bank closed、row geometry、初期0、読書きbyteとbusy/queue、完全memory recordと要求行を照合する。期間の単位はpsであり実DDR部品の代表値とは扱わない |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0051"></a>

## 2. DDR境界・容量・停止

```trace
{
  "id": "DIR-TEST-0051",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0198",
    "DIR-REQ-0199",
    "DIR-REQ-0200",
    "DIR-REQ-0201"
  ],
  "upstream": [
    "design-memory-ipc#ddr"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#ddr)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#ddr) |
| 上流 | [DIR-FUNC-0047](../../機能仕様書.md#dir-func-0047)、[DIR-AC-0048](../../要件定義書.md#dir-ac-0048) |
| 入力 | ddr-stop、ddr-errorsのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 期待 | T7は初回write active、completion=null、memory全0。capacity2の同時3要求は先頭2受理/第3 queue_full、fault対象の第2は6psにmemory_fault。行跨ぎとsize外はaddress_error |
| 構成境界 | invalid-configsのgeometry_size/refresh_intervalを拒否。size最小/最大、初期範囲重複、幅非整合、D非正規表記、時刻overflowとunknown keyを製品prepare試験で確認 |
| 停止・失敗注入 | 製品ではcompletion/refresh due/start/endのT−1/T/T+1、queue_capacity0、同時dueとcomplete、write commit予約失敗を確認し直前journal byte像だけを保存する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0052"></a>

## 3. SRAM二port・同時可視化

```trace
{
  "id": "DIR-TEST-0052",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0202",
    "DIR-REQ-0203",
    "DIR-REQ-0204",
    "DIR-REQ-0205"
  ],
  "upstream": [
    "design-memory-ipc#sram"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#sram)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#sram) |
| 上流 | [DIR-FUNC-0048](../../機能仕様書.md#dir-func-0048)、[DIR-AC-0049](../../要件定義書.md#dir-ac-0049) |
| 入力 | sram-portsのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 入力 | 2port、read/write4ps。a write0..1=aabbとb read0..1を時刻0、c write1=ccを時刻1に生成 |
| 期待 | port0/1/0、grant0/0/4、completion4/4/8ps。4ps batchはa先行でb結果aabb、最終memory aacc後続0、write3byte。busy和12ps、分母2*H=18ps |
| 独立性 | 製品ではNED登録順とconfig配列順を反転して同じordinal/結果を確認。異なる完了時刻のread/write順序と同時write重複も追加し決定的な最終内容を検査する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0053"></a>

## 4. SRAM境界・停止prefix

```trace
{
  "id": "DIR-TEST-0053",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0202",
    "DIR-REQ-0203",
    "DIR-REQ-0204",
    "DIR-REQ-0205"
  ],
  "upstream": [
    "design-memory-ipc#sram"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#sram)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#sram) |
| 上流 | [DIR-FUNC-0048](../../機能仕様書.md#dir-func-0048)、[DIR-AC-0049](../../要件定義書.md#dir-ac-0049) |
| 入力 | sram-stopのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 期待 | T8ではc writeはactiveで完了null、最終memory aabb後続0、commit2byte。port積分は12ps/(2*8ps)。c予定完了8psを実績へ転記しない |
| 境界 | invalid-configsのports0を拒否。製品でports1/16、queue0、size外、fault範囲、重複初期byte、read/write時刻overflowを確認。faultは通常応答でportを占有しない |
| 失敗注入 | 製品で同時completion batchの最終serializer/予約失敗を注入し、batch中間writeが結果へ漏れず旧byte像だけを保存する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0054"></a>

## 5. 共有IPC所有権・公開

```trace
{
  "id": "DIR-TEST-0054",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0206",
    "DIR-REQ-0207",
    "DIR-REQ-0208",
    "DIR-REQ-0209"
  ],
  "upstream": [
    "design-memory-ipc#shared"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#shared)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#shared) |
| 上流 | [DIR-FUNC-0049](../../機能仕様書.md#dir-func-0049)、[DIR-AC-0050](../../要件定義書.md#dir-ac-0050) |
| 入力 | shared-ownershipのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 期待 | publish0→3psでbeef公開、4psの次publishはfull。consume5→10psでbeef取得/free化。11psのconsumeはempty、6psのintruderはaccess_denied。published=consumed=1 |
| 確認 | slot最小index、ready FIFOと状態対応、publish完了前payload非公開、消費中owner=consumer、free後hex空を確認。製品で2slot以上のFIFOと複数actorでも同じ規則を照合する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0055"></a>

## 6. 共有IPC停止・権限・容量

```trace
{
  "id": "DIR-TEST-0055",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0206",
    "DIR-REQ-0207",
    "DIR-REQ-0208",
    "DIR-REQ-0209"
  ],
  "upstream": [
    "design-memory-ipc#shared"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#shared)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#shared) |
| 上流 | [DIR-FUNC-0049](../../機能仕様書.md#dir-func-0049)、[DIR-AC-0050](../../要件定義書.md#dir-ac-0050) |
| 入力 | shared-stopのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 期待 | T3でslot publishing/owner producer/slot hex空、publish0件、completion null。予定publish3を既公開と扱わない |
| 境界 | invalid-configsのslots0を拒否。製品でconsume completion=T時のconsuming/owner/公開byte保持、容量超過、空queue、同時publish完了→consume、producer/consumer役割交換によるaccess_deniedを確認 |
| 保存則 | 製品でpublished=consumed+ready+consuming、free/publishing/ready/consumingの総数=slots、待機容量と占有slotを別計数する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0056"></a>

## 7. DMAと実memory競合・内容

```trace
{
  "id": "DIR-TEST-0056",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0210",
    "DIR-REQ-0211",
    "DIR-REQ-0212",
    "DIR-REQ-0213"
  ],
  "upstream": [
    "design-memory-ipc#dma"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#dma)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#dma) |
| 上流 | [DIR-FUNC-0050](../../機能仕様書.md#dir-func-0050)、[DIR-AC-0051](../../要件定義書.md#dir-ac-0051) |
| 入力 | dma-memory-contentのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 入力 | chunk4byte、setup2ps、notify2ps。SRAM初期0102030405060708からDDRへ8byte copy。SRAM read3/write4ps、4psの直接writeがsource後半をf0f1f2f3へ変更 |
| 期待 | read完了5/15ps、DDR write完了12/22ps、data_done22ps、通知24ps。最終destination01020304f0f1f2f3、committed8byte。source変化が第2read snapshotへ反映する |
| 連携 | child IDとorigin親、memory要求のFIFO/port/row/refresh経由を確認。製品でDDR→SRAM、同一memory非重複、複数engineとdirect traffic、DDR行境界chunk分割を実施する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0057"></a>

## 8. DMA部分停止・fault・通知

```trace
{
  "id": "DIR-TEST-0057",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0210",
    "DIR-REQ-0211",
    "DIR-REQ-0212",
    "DIR-REQ-0213"
  ],
  "upstream": [
    "design-memory-ipc#dma"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#dma)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#dma) |
| 上流 | [DIR-FUNC-0050](../../機能仕様書.md#dir-func-0050)、[DIR-AC-0051](../../要件定義書.md#dir-ac-0051) |
| 入力 | dma-stop、dma-fault-prefix、dma-offer-orderのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 期待 | T22で第2writeは未commit、parent writing、committed4byte、destination01020304後続0、data_done/通知null。dst offset4 faultは第2write admission15psでparent failed/child_memory_fault、完了15ps、同じ4byte prefixを保持 |
| 同時admission | dma-offer-orderはsetup終了2psのchild（root z）と外部read aを同じSRAMへofferする。phase1で両方収集し、phase2でa→zの順にadmission。capacity1によりaが受理、childはqueue_full、親は2psにfailed、commit0byte。空portの有無によらず全admission後にdispatchする |
| 境界 | invalid-configsのchunk0を拒否。製品で未配置src/dst、範囲外、同一領域重複、child queue_full、notify時刻=TとT+1、最終writeとparent進捗journal同時保存を検証する |
| 失敗注入 | 製品でread後write予約失敗、writeとparent進捗のcommit失敗を注入。成功prefixを保存し、既writeの二重計上や未writeのcommitted増加を拒否する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0058"></a>

## 9. mailbox FIFO・通知

```trace
{
  "id": "DIR-TEST-0058",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0214",
    "DIR-REQ-0215",
    "DIR-REQ-0216",
    "DIR-REQ-0217"
  ],
  "upstream": [
    "design-memory-ipc#mailbox"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#mailbox)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#mailbox) |
| 上流 | [DIR-FUNC-0051](../../機能仕様書.md#dir-func-0051)、[DIR-AC-0052](../../要件定義書.md#dir-ac-0052) |
| 入力 | mailbox-fifoのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 入力 | capacity1、service2/notify5ps。send a/bを0ps、receive cを3ps、receive dを7ps |
| 期待 | 応答2/4/6/9ps、ok/full/ok/empty。cはa:0のcafeを取得。aの通知予定/実績7psで、すでに6psに受信済みでも一度通知。sent=received=1、queue0 |
| 計測 | successful enqueueとreceive、notificationを別計数し、応答遅延と通知遅延を混同しない。製品で複数message FIFOと全receiver actorへの通知宛先を検証する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

<a id="dir-test-0059"></a>

## 10. mailbox停止・有限容量

```trace
{
  "id": "DIR-TEST-0059",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0214",
    "DIR-REQ-0215",
    "DIR-REQ-0216",
    "DIR-REQ-0217"
  ],
  "upstream": [
    "design-memory-ipc#mailbox"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 担当 | [設計](../../design/メモリ・IPC詳細設計書.md#mailbox)、[仕様](../../specs/models/メモリ・IPC詳細機能仕様書.md#mailbox) |
| 上流 | [DIR-FUNC-0051](../../機能仕様書.md#dir-func-0051)、[DIR-AC-0052](../../要件定義書.md#dir-ac-0052) |
| 入力 | mailbox-stopのINIと[構成境界](../fixtures/memory-ipc/invalid-configs.json) |
| 期待 | T7でpayloadは受信済みqueue0、sent/received各1、notify予定7ps/実績null。send成功を未通知のため取消さない |
| 境界 | invalid-configsのcapacity1025を拒否。製品でcapacity0の全send full、queue_capacity0の全offer rejected、send/receive権限違反、payload最大値、同時刻receiveと次send順を確認 |
| 失敗注入 | 製品でnotification予約失敗時はsend enqueueと通知行をともに未commitへ戻し、publish済みjournalの保存則とresponse件数を確認する |
| 結果 | 解析fixture照合済み。製品試験・内部検査は未実施 |

## 11. 追跡と受入の完了条件

| 項目 | 内容 |
| --- | --- |
| 追跡 | `python3 scripts/check_traceability.py --strict --requirement DIR-REQ-0198`を0198～0217の各IDへ適用し、FUNC→詳細仕様→architecture#arch-memory-ipc→詳細設計→TESTの実在経路を確認する |
| 受入 | 5件のACごとに上記正常・境界両TESTを製品で実行し、全fixture projectionと内部確認を満たした時点で製品受入とする。解析PASSだけで製品合格へ更新する処理は対象外（注） |
