# AXI5アトミック・QoS詳細設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
公開状態：v1.2.0収録。実装台帳に記録した契約・検証範囲を参照。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

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
