# SoC実行詳細設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
公開状態：v1.2.0収録。実装台帳に記録した契約・検証範囲を参照。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

文書ID：`design-soc-execution`
文書状態：実装対応草案。受け入れ契約と実行済み証跡を区別する。

<a id="execution"></a>

```trace
{"id":"design-soc-execution#execution","stage":"design","requirements":["DIR-REQ-0295","DIR-REQ-0296","DIR-REQ-0297","DIR-REQ-0298"],"upstream":["architecture#soc-execution"],"state":"confirmed","pending":[]}
```

## 唯一のtimeline

`runtime/soc_execution/mod.rs`がModel/Context/FES timerとMemoryServiceを所有する。`cpu.rs`は純粋decoder/evaluator、`coherence.rs`は有限directory/cache、`ipc.rs`はDMA/lease、`transport.rs`はembedded adapter、`ddr.rs`はasynchronous physical targetを担当する。stateはcallback candidateへcloneし、record、next timer、invariant検査が成功した境界で公開する。

native AXI/AHB step時だけsole MemoryServiceを元engineへ移し、同callback内で返す。embedded serviceが保存するmemoryは空であり、別simulation、別scheduler、shadow RAM、既計算traceの結合を使わない。NoCは元Network.tickを直接呼ぶ。global effect sequenceを全engineに渡す。

## 完了境界

CPU fetchは長さ4の実Read。実responseが返るまでdecodeしない。load/storeは評価したpending Actionとtransactionを結び、response後だけregister/PCをretireする。cached hitもdirectory lockを取る。待機中の別writerによるinvalidate後はhitを再評価し、missに変わった場合は実transportへ戻す。

directoryはline別epochとtransient txnを持つ。ACKはcore/epoch/timeを照合し、dirty byteはinterventionへ保存する。writeback転送の完了後だけclean downgrade/evictionを公開する。grantは4bit QoS+上限付きaging、oldest/source/ID/sequence順で決定する。grant_orderをnative submissionへ引き継ぐ。

DDR bridge jobはphysical Request identity、native targetまたはcoherence owner、各byteのseen、capture、完了/errorを持つ。AXI writeはAWと対応Wの実受理後に有限buffer内のbyteをbackendへ渡す。AHBは実data phaseで要求する。実半cycleeffectのdeadlineをlocal core/NoC/native progress deadlineとmergeし、700us初期化を1ps pollingしない。

## restoreの拒否条件

`checkpoint.rs`は不変入力、x0、CPU phase/action/request/PC/width、成功response長、DMA descriptor/offset/staging、cache/directory owner/sharer/epoch/ACK、有限resourceを照合する。native job IDは`txn*4097+offset`からlogical chunkを再構成し、source/address/length/op/byte/strobeと結び付ける。Native phaseはlive native jobまたは実captureに一致するqueued completionを必須とする。NoC packetの実payload、endpoint、request/response lifecycleもlogical txnと相互照合する。

DDR Service.validate、bridge job/active request bijection、initialized-byte accountingを照合する。単一live timerはTimerReferenceと実FES Envelopeのdeadline/schema/payload/source/destinationを照合する。loadはtimerを再予約しない。

## mapped targetと復元時の意味検証

`mailbox.rs`はsource別staging、endpoint別応答保持slotとaccepted accessを保存する。AXI/AHBの`AsyncTarget`はRAMを保持せず、実ARまたはAW+W、AHB data phaseの受理後だけ4byte target requestを返す。物理byte配列とnative responseのcaptureは、同じaccepted request identityへ結び付ける。response backpressureの間もapplied markerを保持し、同じCOMMANDを再実行しない。

`ipc.rs`の純粋なlease開始・完了・状態遷移と、`mailbox.rs`の純粋なregister操作を、通常実行と`ipc_checkpoint.rs`の検査で共有する。検査対象はboundedな制御metadataであり、CPU・bus・DDR・payload RAMを再実行しない。journal originは不変のscript command index、mapped transaction、DMA publish descriptor、実DDR RESET commandへ結ぶ。transactionのadmission/completion sequenceと合わせた順序で、slot phase/generation/owner/consumer、完了byte/strobe、active read、staging、RESULT、register read、通知を再構成する。

journalとapplied marker、native target完了、logical responseを双方向に照合する。未publishのReady化、空lengthへの改変、通知の削除・復活、register read/capture/resultの同時改変、欠落・重複originを拒否する。atomicは同一source/IDの先行requestがdrainしてから共有ownerを予約し、endpoint待ちの先行通常requestとの相互待ちを避ける。
