# AXI5アトミック・QoS詳細設計書

文書バージョン：`1.1.0`（初版予定）
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
公開状態：未commit・未公開の実装作業文書。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 単一ownerによる実行・転送・復元契約を追加 |

文書ID：`design-axi5`
文書状態：実装対応草案。受け入れ契約と実行済み証跡を区別する。

<a id="atomic"></a>

```trace
{"id":"design-axi5#atomic","stage":"design","requirements":["DIR-REQ-0300","DIR-REQ-0301"],"upstream":["architecture#axi5"],"state":"confirmed","pending":[]}
```

## 実装

input/soc_execution.rsでAWATOP、32bit alignment、operand length、Compare layout/burst、4bit QoSをstrict検査する。runtime/soc_execution/memory.rsの純粋演算はcaptured old wordとoperandからnew wordを作る。coherence ownerは同一lineの他CPU/DMA/manager操作を排他し、実write完了後にlinearization recordを出す。DDRならWriteByte/RequestCompleteを待つ。

transport.rsは永続axi_write_owner、AW受理、W受理byte数、R送信byte数、B送信済みをcheckpointする。QoS仲裁は途中W beatのownerを変更しない。RとBは独立ready時刻を使い、両方が必要な操作は両handshake後だけlogical completionへ進む。native ordinary AXIの共有outputも元FabricでAW順序と非interleave W順序を一致させる。

admission cursor、admitted時刻、grant_orderを保存する。effective QoSを時刻から再計算し、grant orderをnative service submissionへ引き継ぐ。owner/resource/response payloadの検査はSoC共通codecを用いる。
