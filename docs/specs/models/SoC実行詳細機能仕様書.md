# SoC実行詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
公開状態：v1.2.0収録。実装台帳に記録した契約・検証範囲を参照。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

文書ID：`spec-soc-execution`
文書状態：実装対応草案。受け入れ契約と実行済み証跡を区別する。

<a id="execution"></a>

```trace
{"id":"spec-soc-execution#execution","stage":"spec","requirements":["DIR-REQ-0295","DIR-REQ-0296","DIR-REQ-0297","DIR-REQ-0298"],"upstream":["DIR-FUNC-0080"],"state":"confirmed","pending":[]}
```

## 実行契約

`soc.execution.v1`は単一registered Model `dir.soc.Execution`、schema 3出力を持つ。型は`crates/dir-simulator/src/lib/types/soc_execution.rs`、入力はmodel `dir.soc-execution.model` / workload `dir.soc-execution.workload` version 1。既存`soc.shared.v1`を維持する。

CPUはlittle-endian RV32I 2.1の命令wordを実共有memoryからfetchする。LUI/AUIPC/JAL/JALR、六分岐、LB/LH/LW/LBU/LHU、SB/SH/SW、整数即値・register演算、FENCE、ECALL/EBREAKをdecodeし、x0固定・32bit wrap・符号比較・シフト下位5bitを適用する。instruction/data misalignment、権限・decode違反、未定義命令、ECALL/EBREAKはprecise trapで該当命令をretireしない。privileged ISA、CSR、M/A/C拡張は含めない。FENCEは全共有未完了操作を保守的にdrainする。命令budgetは停止理由を`cpu_budget_paused`として出し、正常終了命令へ読み替えない。

## 有限cacheとdirectory

各coreは有限set/wayのwriteback cacheを持つ。lineはS/E/M、非存在をIとする。directoryはowner、sharers、epoch、GetS/GetM/evict transientを持ち、有限MSHRを消費する。同一lineの所有権を一つに限定する。GetMは他owner/sharerへinvalidateを送り、対応epochのACKとdirty intervention bytesの完了を待つ。GetSはdirty ownerの実byteをwritebackしてSへdowngradeする。dirty evictionはbyte転送とACK完了までvictimを再使用しない。外部DMAとuncached managerも同じdirectory exclusionを通り、古いcache値を観測しない。

## transportとbyte owner

memory mapは重複しないread/write/execute権限付きregion。SRAMと選択DDR regionは一つのMemoryServiceだけにbyteを保存する。AXI adapterは既存`axi4.concurrent.v1`のchannel engineを直接stepし、line内をsize=1のINCR burstへ変換する。AW/W/R/Bの実handshake、WSTRB、有限W buffer、同じ共有出力のAW順序・非interleave W、ID、response backpressureを保持する。AHB adapterは既存`ahb.pipeline.v1`を直接stepし、実byte SINGLEのaddress/data phase、HREADYとERROR応答を使う。NoC adapterは既存`noc.flit.v1`の同じflit/VC/credit engineに実request/response payloadを流す。cache fillは実response到着後だけ行う。

DDRは選択ISSI mode/source identityを要求し、preloadを拒否する。controllerはRAMを持たず、実WriteByteだけがMemoryServiceを変え、実ReadByteだけがcaptured responseを作る。R/B/HREADY、dirty writeback、evictionはphysical RequestCompleteを待つ。未初期化・reset消失byteのreadはerror。詳細はDDR4 family仕様を正本とする。

## DMAとlease

DMAは有限staging bufferへ実source read結果を取り込み、実destination writeとcoherence完了後だけoffsetを進める。cancel後に未完了データをpublishしない。IPC slotは物理rangeが互いに重複せず、Free→Leased→Filling→Ready→Reading→Freeをgeneration/ownerで保護する。全byteの実write完了と対応live transactionのdrain後だけpublishとnotificationを出す。早期publish、異owner、古いgeneration、active read中のreleaseを拒否する。workload commandとmapped mailboxは同じlease状態遷移を使用する。

## 継続と範囲

cooperative control/checkpointを提供し、metrics v2は未提供。実CPU retire、stall、ACK、writeback、transport record、visibility、lease transition、DDR effectを同一timelineへ記録する。時刻limitでは未完了操作を完了へ補わない。codecはpending命令、cache/directory、DMA capture、native bus/NoC state、byte owner、DDR pendingbyteと単一owned timerを保存する。電気的・全AMBA/RISC-V規格適合、privileged実行、cycle-accurate実CPU性能の主張はしない。

## メモリマップド・メールボックス ABI v1

任意の`mailboxes`配列で`id/base/slot/producer/consumer/service_cycles`を指定する。1 endpointは既存slot 1個に対応し、producerとconsumerを不変のsource IDで区別する。制御領域は64byteで、AHB subordinateの最小割当てに合わせた1KiB decoder内へ置く。同じdecoder内に複数endpointを配置できる。decoder全体とRAM/DDRの重複を拒否し、未使用アドレスはerrorとする。payload byteを制御領域へ複製しない。

| offset | 32bit little-endian register | アクセス |
| --- | --- | --- |
| `0x00` | STATUS: phase 0=Free/1=Leased/2=Filling/3=Ready/4=Reading、bit9=直前COMMANDのerror | RO |
| `0x04/08` | 現在generationの下位/上位 | RO |
| `0x0c` | 次のlease length | RW、source別staging |
| `0x10/14` | expected generationの下位/上位 | RW、source別staging |
| `0x18` | COMMAND: 1=LEASE/2=PUBLISH/3=BEGIN_READ/4=RELEASE | WO |
| `0x1c` | RESULT | RO |
| `0x20/24` | 最後に成功したpublishのgeneration下位/上位、通知なしは0 | RO |

RESULTは0=成功、1=異source、2=古いgeneration、3=phase違反、4=未完成payload、5=未完了操作、6=length違反、7=未定義COMMAND。意味的に失敗したCOMMANDもレジスタ転送自体は正常完了し、RESULTで判別する。shape/権限違反はnative SLVERR/ERROR相当で完了し、staging/slotを変えない。

レジスタアクセスは自然整列した32bit single beat、writeは全4byteのstrobeだけを受け付ける。部分・zero strobe、未整列、複数registerをまたぐburst、atomic、fetchを操作全体で拒否する。CPU dataアクセスはuncachedであり、cache fillやDDR commandを発生させない。COMMANDは実target受理時のsource別staging tupleをsnapshotし、設定service latency後に一度だけ適用する。endpointの応答保持slotは1個で、native responseがdrainするまで次のアクセスを待たせる。STATUS読出しも同じ順序に従う。

通常payloadのnative byte adapterと区別し、レジスタはAXI size=4/len=1、AHB HSIZE=4/SINGLEの実transferで運ぶ。NoCは実packetの受信後にtargetを起動する。DMAによるregister操作もdescriptor全体が整列4byteかつbuffer capacityが4byte以上の場合だけ受け付け、分割によるCOMMAND化を防ぐ。DDR-only RESETはDDR-backed slotと通知を無効化し、SRAM mailboxのpending native応答をabortしない。全体の再生成時はstaging/RESULT/通知を初期化する。

consumerの通常DMA source readは共有RAM読出しであり、Requestのlease readを自動的に付与しない。複数chunkを含むconsumer DMAでは、callerが実DMA完了を待ってからRELEASEする。consumer DMA全体を自動pinするdescriptor APIはこの契約に含めず、その寿命保護が必要な操作には明示的なlease-bearing Requestを用いる。
