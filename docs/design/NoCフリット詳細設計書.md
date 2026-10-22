# NoCフリット詳細設計書

文書バージョン：`1.1.0`（初版予定）
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
公開状態：未commit・未公開の実装作業文書。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | registered Modelの有限buffer/credit実装と復元検査を追加 |

文書ID：`design-noc-flit`
文書状態：実装対応草案。

<a id="network"></a>

```trace
{"id":"design-noc-flit#network","stage":"design","requirements":["DIR-REQ-0291","DIR-REQ-0292","DIR-REQ-0293"],"upstream":["architecture#noc-flit"],"state":"confirmed","pending":[]}
```

## 所有権と状態

[仕様](../specs/models/NoCフリット詳細機能仕様書.md)の唯一ownerは[Network](../../crates/dir-simulator/src/runtime/noc_flit/mod.rs)。不変Topology、Config、正規化Workloadを所有し、可変Stateへpacket進捗、healthy link、routing identity、epoch、Buffer、Transit、Credit、source/ejection queue、仲裁cursorを保存する。家族独自のschedulerや私有memory serviceは持たない。

物理入力buffer番号は`link_id*(adaptive_vcs+1)+vc`。その後へrouter別の有限local injection bufferを置く。Bufferはpacket owner・FIFO・downstream reservationを持つ。Transitは実flit・送信元VC・宛先VC・arrival時刻を持ち、Creditは返すbuffer・到着時刻・epochを持つ。Flitは不変packet参照・index・epochであり、payload生成で補完しない。packetのroute historyとtailのrelease prefixを保存し、所有権の孤立を検出する。

[Topology](../../crates/dir-simulator/src/runtime/noc_flit/topology.rs)はparallel portを保持してreverse linkをport方向で対応付ける。各宛先の最短距離、各成分BFS parentとtree link集合を決定的に再構成する。escape pathはtreeだけで探索する。adaptive→escapeの一方向移行とup→downの木依存により、escape channel依存は循環しない。

## callbackと失敗原子性

`dir.noc.FlitTick` version 1をphase 1のowned timerで受ける。[router](../../crates/dir-simulator/src/runtime/noc_flit/router.rs)はStateをcloneしたcandidateに対して仕様順の処理を行う。全出力linkのproposalを一つのsnapshotから作り、link別round-robinでgrantし、同じ物理input portの二重readをejectionも含め防ぐ。実launch後のarrivalは最低一cycle後となる。

時刻演算、record upsert、次timerの予約、全状態invariantの検査が成功した場合だけcandidateとtimerを公開する。Contextのcallback rollbackと合わせ、失敗時に部分的recordとmodel stateが食い違わない。完全idleなら最初のfuture packet/fault時刻へ直接進み、credit returnが残っている間は実token処理を続ける。

## checkpoint検査

[checkpoint](../../crates/dir-simulator/src/runtime/noc_flit/checkpoint.rs)はcodec/policy/config/workload identityを照合する。workloadのfault列から現在healthy状態とepochを再演算し、distance/tree identityと比較する。全flitは一意のpacket/indexを持ち、injected済みかつ未消費のflitをFIFO/pipeline/ejectionにちょうど一つずつ要求する。ownerとroute/release prefix、予約と隣接link、FIFO順序、最短adaptive path、escape treeと移行方向を検査する。古いepoch、過去arrival/credit、credit容量式、有限queue上限、payload prefixとterminal accounting、source pending、cursorとnext tickを拒否条件へ含める。

timerは既存の明示TimerReference APIでencode/decodeし、model ownerのlive timerが一つであることを要求する。load自体ではtimerを再予約しないため、kernelから戻したFESと競合せず、既出recordを繰り返さない。
