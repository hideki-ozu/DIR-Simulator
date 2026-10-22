# TSN集中設定詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-qcc-cnc`
文書状態：未commit・未公開。本契約のfocused製品gate・独立レビュー済み。

<a id="configuration"></a>

```trace
{"id":"design-qcc-cnc#configuration","stage":"design","requirements":["DIR-REQ-0302"],"upstream":["architecture#qcc-cnc"],"state":"confirmed","pending":[]}
```


### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 集中設定・受入れ判定・実ゲート適用と継続契約を追加 |

## 所有と型

`lib/types/network/configuration.rs`はCUC要求、operation、prepared topology/resource、Planの型を持つ。`input/network/configuration.rs`で既存PreparedNetworkの実port/classification/capacityへbindし、別のlink/byte/queue ownerを作らない。

`runtime/network/configuration.rs`のQccStateは有限管理・契約service。version、topology epoch、active/previous/pending、participant vote、operation cursor、source counter、transition auditを保持する。packetを送信せず、private schedulerも持たない。NetworkState/NetworkDeltaが共通FESとreserve/applyの単一publish境界を所有する。

## planner

頂点は`(direction,class)`、辺は同じstreamの連続hop。頂点windowの幅はframeごとの切上げ占有時間×最大burstと両側jitter/clock-error margin。先行window終端と伝搬・保守的な送受信処理を超え、port上の既存windowと重ならない最初の位置へ配置する。sourceのearliest/latest offsetと最終interval原点boundを検査する。有限search budgetを消費し、dependency cycleや未対応shared classを区別する。

reserved class集合は全CUC requirementから不変に決める。planは撤去で空になったmanaged portも含む。未束縛frameは途中のuntagged ingressでpriorityが変わってもreserved classへ入れない。offerは`qcc_unbound_class`を記録し、CUC source bindingのないcopyをqueue投入前に拒否する。背景queueのadmissionで`total+1+sum(reserved-used) <= aggregate capacity`を検査し、通常のclass別frame/byte制約も維持する。on-wire frameを含めるかは既存queue capacityの定義と一致させる。

## 実runtime adapter

`QccState::advance`は正確なowned control deadlineで実行し、旧候補のACK/timeout/activationを同時刻の新しいoperationより先に処理する。管理deadlineを飛ばす呼出しを拒否する。`admit_source`は未来のReady時刻をGenerate時に先取りせず、実Readyで呼ぶ。同時刻のcontrolを先に反映し、Readyを同時刻のTAS/arbitrateに間に合わせる。

frameにgenerator/stream/version/interval原点/source Ready/payloadをbindする。sourceとrelayのroute選択は通常のflood/FDB fallbackより先にCNC pathを使うが、実ingress/egress eligibilityを省略しない。control actorは実DynamicStateのpolicy viewを参照し、CNC専用の架空topologyを動かさない。

`schedule`はPlanを既存Schedule型へ変換する。`TsnState::plan_install_in`はstaged_port、advance_port、finish_portを使い、全portの新GCLとwake generationを一つのDeltaへ重ねる。送信中portとbase timeの不一致を拒否し、Network側でqueued/propagating/processingも含めたdrainを検査する。reserve失敗時に片側だけcommitしない。

## checkpoint

QccStateのauditを小さな管理serviceで再適用し、active/pending/version/source counter/certificateの一致を検証する。packet simulation全体をreplayするものではない。source auditは実accepted source frame、topology auditは実policy control、quiescenceは実activation/drainの履歴へ独立にbindする。auditとstateを同時に書き換えた自己整合だけを信頼しない。

`installation_history`から初期の閉じたschedule、成功activation、suspend時の閉鎖を再生成し、TsnStateのoperating Scheduleとgenerationを復元する。serialized state内に新たなscheduleの正本を持たない。共通FESのowned timer envelopeとQcc next_deadline、ACK/timeout/cancel generationを照合し、古いwakeが候補を復活させない。
