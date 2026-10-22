# TSN集中設定検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`verification-qcc-cnc`
文書状態：未commit・未公開。本契約のfocused製品gate・独立レビュー済み。

<a id="dir-test-0141"></a>

```trace
{"id":"DIR-TEST-0141","stage":"verification","requirements":["DIR-REQ-0302"],"upstream":["design-qcc-cnc#configuration"],"state":"confirmed","pending":[]}
```


### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 集中設定・受入れ判定・実ゲート適用と継続契約を追加 |

## serviceとproductionの区別

`cargo test -p dir-simulator --lib qcc_ --offline`は有限planner、整数ceil/padding/IFG、path処理、buffer reservation、source契約、ACK/reject/silent/timeout/cancel/busy、drain拒否、topology、rollback、改変state拒否を検証する。生成Scheduleを既存TsnStateへinstallして実selectionが閉/open時刻に従うことと、managed Scheduleのcheckpoint再生成も対象にする。MaxLatency=0のbound発見と境界/1ps不足を別に検査する。

serviceの成功をT04/T05のproduction完了にしない。`tests/network_qcc.rs`と`examples/network-composite/qcc/`で、実route、queue reservation、生成GCLによるSOF、actual receiver byte、interval原点からの完了boundを確認する。本契約のruntime統合とfocused acceptance gateを完了した。リポジトリ全体の最終gateは別に行う。

## 必須gate

- 二つの単独では受理可能なstreamの競合と、背景frameが最大限queueを占めた場合の実admission。
- inactive/oversize/excess/late sourceの拒否、source processing中のversion切替、offsetと同時刻のReady/control/TAS順序。
- 普通のstatic routeと異なる明示path、tagged egress、untagged ingressのclass維持、実SOFとlistener byte/bound。
- timeout/reject/cancel/busyで旧version/route/GCLを維持し、撤去・再追加・rollbackと実topology更新を確認。
- 切替時の旧queued/on-wire/propagating/processing frameでdrainを拒否し、backgroundの最後の許可SOFがmanaged windowへはみ出さないこと。
- pending候補中とactivation後の別process checkpoint継続一致。正しいrehashを対照としてgate/version/counter/wake/source/topology/drain履歴の改変を意味検査で拒否。
- Viewerで準備/ACK/拒否/適用/suspendと実転送を区別し、停止prefixに未来の適用や受信を補わないこと。

## 判定

探索budgetや対象外の組合せの拒否は、物理的な不可能性の証明ではない。accepted streamの保証は明示したcapacity、source、clock、topology仮定が有効な期間に限定する。合成制約の残るN13、外部UNI wire、IEEE認証の完了をこの検証から導かない。

## 最終focused gateとレビュー

有限service/TASの7件と、実製品統合の5件が通った。baselineは95イベント・518記録・4件の実配送で、全件がcertificate内だった。保存継続は5停止点とsource/TAS/audit等の意味改変を確認した。独立レビューで見つかった背景trafficのingress再分類による予約class流入は、source bindingをqueue admission前に検査して修正した。実untagged入力のdefault priority0では通常配送し、5では`qcc_unbound_class`でSOF前に拒否する。双方で4件の予約配送を保持する。

[最終独立レビュー](../results/remaining-20261021/qcc-final-closure-review.md)は最後の指摘を閉じた。本契約はローカルCUC/CNC APIであり、wire UNI相互運用やIEEE全体認証ではない。
