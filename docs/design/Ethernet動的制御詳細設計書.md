# Ethernet動的制御詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 2f1e60b`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）
文書ID：`design-ethernet-dynamic`
文書状態：未公開。抽象モデルを実装済みで、native CLI・schema2出力・Viewerに対応する。実施した製品試験と未照合の組合せは検証仕様の実施記録に区別して示す。規格全体適合の証明ではない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | 初回pushに向け、動的membership・VID登録・学習・tree・制御stateの所有と原子的commit・観測の詳細設計を確定 |

## 1. 所有境界

[詳細機能仕様](../specs/models/Ethernet動的制御詳細機能仕様書.md)を入力・動作の正本とする。[既存VLAN設計](EthernetVLAN・マルチキャスト詳細設計書.md)のserializer、wire別容量、queue、parent chain、結果writerを再利用する。公開Registryのprofile/module/event/record descriptorとprepare経路へ接続する。全体のFESや旧Ethernet profileを置換しない。`runtime/network.rs` の一つの `NetworkState` に `DynamicState` と共有Ethernet L2 queue/wire stateを置き、`runtime/registered.rs` の共通Engine/FESへ接続する。dynamic childはFESを持たない。

<a id="membership"></a>

## 2. 型・prepare・control処理

```trace
{"id":"design-ethernet-dynamic#membership","stage":"design","requirements":["DIR-REQ-0240","DIR-REQ-0241","DIR-REQ-0242","DIR-REQ-0243"],"upstream":["architecture#arch-ethernet-dynamic"],"state":"confirmed","pending":[]}
```

### 2.1. 型

| 型 | 内容・所有 |
| --- | --- |
| `PreparedDynamicEthernet` | bridge/link番号表、registrable、正規control列、IP metadata、有限key集合、limits。既存VLAN設定は `PreparedNetwork.ethernet` に保持。prepare後immutable |
| `IpMulticast` / `MembershipKey` | family enumと正規 `IpAddr`。source/groupはframe metadata、membershipはswitch/VID/family/group/port key |
| `ControlOp` | MembershipSet/Leave、RouterSet/Leave、VlanRegister/Unregister、LinkSetの厳密enum。共通id/timeと入力位置を保持 |
| `DynamicState` | static参照、BTreeMap MacKey→Learned、MembershipKey→Filter、RouterKey→Lease、RegistrationKey→Lease、link状態、tree port role、policy_epoch |
| `Lease<T>` | expires_at、generation、value。世代は削除後もkey別generation ledgerに保持し再作成で再利用しない |
| `Partition<K,T>` | table別の期限→key indexとkey別generation ledger。membership/router/MAC/registrationを別partitionに保持 |
| `Topology` | topology_generation、現在公開roles、収束予定時刻、target graph。rolesと予定treeを区別 |
| visit / copy ID | run内u64単調、frame内visit_count、parent_copy_id。文字列出力は `frame_id/v<visit>/c<copy>`、sourceはvisit0 |
| `DynamicDelta` | 変更前条件、mapの挿入/削除、期限の取消/追加、dirty outputs、records、epoch増分と資源差分 |

削除keyの世代台帳は無制限に増やさない。prepare済みcontrol keyの集合とMAC key集合に限り所有する。MAC keyの候補は全Endpoint MAC×出現可能VID×Switchであり、checked積とメモリ上限をprepare検査する。世代u64加算overflowは診断付き停止。IP addressは構文解析してbyte正規化し、文字列表記違いによる重複を拒否する。

### 2.2. codecと登録

`ethernet.l2.dynamic.v1` をRegistryへ登録し、strict input adapterがcontrol workloadをtyped `ControlOp`へ変換して `PreparedDynamicEthernet.controls` に正規順で凍結する。内部通知は共通 `dir.network.control/1` の世代付きcontrol wakeを使用し、frame入力は `dir.network.event/1` を使用する。coordinatorのID・subjectは `@network`。独立した `ethernet.dynamic.control` event codecは設けず、この名前は監査record schemaに用いる。

`Context::send_request_at`はphase1配送専用のままとし、controlのphase0通知は内部`Context::schedule_at` timerで行う。profile-owned coordinatorが同一tのcontrol/期限/収束を単一batchにまとめる。wire完成処理が供給する完了差分を先に解決し、その後link/STP、registration/membership、aging、最後にTSN GCL差分を適用してからphase1 ingressを配送する。共通Engineのグローバルphase順や他profileのevent順は変更しない。同phase内の生成順任せにcontrolを別々にsendしない。

prepareは全controls（終了T以後も）を型検証し、静的参照/registrableを照合、at+lifetime・at+convergenceとtree costのchecked演算、全量controls上限、key集合サイズを検証する。wire候補・終了時刻の既存検証も継承。control列は正規順で保持し次時刻だけ予約する。timerは期限→keyのindexed mapを用いて更新時に旧期限を除去し、期限bucketが空なら予約取消又は唯一のwakeを更新する。共通FESのtimer取消と世代検査を使い、next wakeだけを予約する。全更新回数分のstale timerを積み上げない。

### 2.3. handlerとcommit

```text
prepare/codec -> current state read -> construct PolicyDelta
 -> check all arithmetic, limits, event reservations, IDs, record capacity
 -> validate Effects against registry/ownership/time rules
 -> commit delta + records + dirty set + reservations
```

committed stateを更新せず `DynamicDelta` を作り、`NetworkDelta` がrecords・dirty ports・期限/処理予約と統合する。`runtime/registered.rs` の `NetworkArena::prepare` はEffectBatchの時刻/所有/参照/ID/容量を検査し `PreparedNetworkEffects` を作る。`NetworkState::reserve` を含む事前予約成功後に、Engine効果と変更済みpartition bufferを同じ確定境界で適用する。公開Model traitにrollback/clone APIを追加しない。失敗時は当該callback前のstate/epoch/record prefixと失敗eventを保持し、以前成功したcallbackを巻き戻さない。

MAC学習は入場判定と同一Arrival transaction内に置く。上限時の学習skipだけは正常結果。control stateの上限・pending timer上限・generation overflow・record予約失敗は制御callback停止。membership/registration満杯でも既存key更新はslot追加不要。control削除no-opは監査行を持ち、取消済み期限や世代不一致wakeは実効監査を出さない。変更ごとにepochを増やすのではなく、一括batchに実効差分が一つ以上あれば一回増やし、同一plan内のcontrol行は同じepoch_before/epoch_afterとbatch内ordinalを持つ。

学習/refreshはexpiry変更もpolicy差分としてepochを進める。期限処理はkey/generation/expiresの三値一致だけを削除する。leave→再joinの旧期限が新joinを消さない。membershipのINCLUDE/EXCLUDE判定はstatic group/routerとのunion後にeligibilityで絞る。VLAN失効はFDB/membershipを暗黙に消さずeligibilityだけを変える。

<a id="topology"></a>

## 3. topology・egress・出力

```trace
{"id":"design-ethernet-dynamic#topology","stage":"design","requirements":["DIR-REQ-0244","DIR-REQ-0245","DIR-REQ-0246","DIR-REQ-0247"],"upstream":["architecture#arch-ethernet-dynamic"],"state":"confirmed","pending":[]}
```

### 3.1. tree計算

Switch間up graphから連結成分を作り、bridge_id最小をrootにして非負costのDijkstraを行う。root portは最小distanceを実現する隣接候補の `(neighbor_bridge_id,neighbor_port,local_port)` 最小。designatedはlink端の `(distance,bridge_id,port)` 最小。正costなのでroot方向にdistanceが必ず減り、両端forwardingの辺はroot treeを構成する。parallel linkもport比較で一意になる。Endpointは転送中継点にせずtree計算から除く。

link値変更時、topology_generationをchecked加算、全dynamic FDBをflush、Switch間rolesをdiscarding、全outputをdirtyとしてconvergence timerを一件設定。連続変更はtargetと期限を更新する。収束期限は同一tのlink_setを先に適用し、旧世代ならtreeを公開しない。同一t複数link_setはid順で最終graphを得て一回だけ再計算/flushし、実際の変更があるときだけ世代を進める。同値link_setはno-op。初期rolesもoutputに保存する。

disconnected graphは複数rootを持つ正当入力。学習・group選択は成分外への存在しない経路を探索しない。copyの進行はon-wireの完了→現在ingress eligibility→processing→固定候補のoffer→現在egress eligibilityの順。processing中のtopology変更では完了時に最新FDB/treeを参照する。offer済みcopyは宛先を再選択しない。

### 3.2. TSN共用API

内部の読み取り専用APIを以下に固定する（公開plugin API追加を意味しない）。

```text
snapshot_policy() -> PolicySnapshot { policy_epoch, topology_generation, roles, link_up, effective_vlans }
eligible_egress(port, vid, snapshot) -> Eligible | Ineligible(LinkDown | StpDiscarding | VlanUnregistered)
policy_epoch() -> u64
```

phase2は一つのsnapshotを取得し、eligibility確認後にTSN gate/credit判定を行う。更新はcoordinatorだけが行う。queue copyにoffer_epochを保持し、開始時にstart_epochを記録する。cached eligibilityはepoch相違で必ず無効化。eligibilityがfalseならgate閉鎖を待たずdropするのでTSNの永久closed gateで無効copyが残らない。APIはMAC/group候補を再計算せず、時間とともに変わるport/VID/STP適格性だけを判定する。

### 3.3. visit IDと原子的fanout

ingress受理ごとにVisitIdを割り当て、copyのparentは到達incoming CopyIdを保持する。sourceはvisit0、以後run単調IDで衝突しない。同一frame/port再訪は別visitとなり、parentが常に以前のcopyを指すDAGを維持する。frame内visit上限を超えるArrivalでは到達receptionを `visit_limit` filterとし、追加forwarding copyを生成しない。候補生成時のcopy ID枯渇はcallback失敗。copy自体のloop TTLで実在しないdropを追加しない。

capacity dropは既存候補別正常結果、queueからpolicy理由でdropするとframes/bytesを一回だけ減らす。wire SOF後はstate変更でcancelしない。EOF/release/arrival予約とon-wire bit積分は維持し、到達filterと送信中断を混同しない。

### 3.4. projection・viewer

外枠output_schema_version=2の既存5ファイルを継承する。固有model_recordsを `ethernet.dynamic.frame/1`、`ethernet.dynamic.transfer/1`、`ethernet.dynamic.reception/1`、`ethernet.dynamic.control/1`、`ethernet.dynamic.policy/1` としてRegistryへ登録し、既存Ethernet schemaの意味・versionを上書きしない。wire/tag/class等は既存typed構造を内包してcanonical projectionする。

control行はcontrol_id、kind、scheduled_ps、applied_ps（未適用null）、batch_ordinal、epoch_before/after、generation、outcome（changed/no_op）、対象key、before/afterを持つ。policyはinitial snapshotとcommit delta。roles/link_up/effective_vlansはEndpointを含む全Ethernet outputを覆う。明示no-opはcontrol監査行、取消済み期限・世代不一致wakeは実効recordなし。固有record.dataの `effect_seq` と時刻/epoch/世代は正規十進文字列で出力する。record/metricもDeltaの一部としてpreflightし、存在しないepoch参照を公開しない。source manifestと全controlsはmetadata側で予定として保持し、未適用を実績model recordで捏造しない。

Viewer loaderはこのprofileと独自schema集合を登録し、ID唯一性・parent前進性・epoch参照・roles・IP family・期限区間を検証する。差分を時刻/commit順に再生してtableを復元し、seekはcheckpoint+prefixから純粋に計算する。BigInt時刻とvisit単位の経路表示を用いる。frameの同port再訪を重複排除せず、quiet port・down linkも表示する。停止snapshotのpolicyが実績commit prefixの再生結果に一致することを検証する。

## 4. 実装配置と検証

パスは `crates/dir-simulator/src` 基準とする。

| 実装ファイル | 内容 |
| --- | --- |
| `input/network.rs`、`input/ethernet/dynamic.rs`、`lib/types/ethernet/dynamic.rs` | strict schema4、全将来control/IP、有限ledger候補のchecked prepare |
| `runtime/ethernet/dynamic.rs` | tree、学習、源別membership、VID登録、期限map、純粋 `DynamicDelta` |
| `runtime/network.rs`、`runtime/registered.rs`、`runtime/ethernet/l2.rs` | 共通FES、phase0 batch、visit/epoch、queue再検査とstaged確定 |
| `lib/snapshot/ethernet/dynamic.rs`、`output/network.rs` | control/policy DTO、schema2、metadata、停止snapshotと集計 |
| `tool/viewer/assets/ethernet-model.js`、`ethernet-app.js` | strict schema/epoch、policy table replay、BigInt seek |
| `tests/network_extensions.rs`、各module単体試験、root `tests/network_viewer_model.test.cjs` | CLI独立時刻、policy/期限/上限、Viewer replay |

`DynamicState::plan_controls` / `plan_learning` は変更するpartitionだけを準備し、`apply` が完成値を交換する。同callbackの学習と転送は `select_egress_after(&DynamicDelta,...)` / `snapshot_policy_after(&DynamicDelta)` で先行差分を読み、committed state全体をcloneしない。finite key ledgerはprepareで最大1,000,000候補・推定256MiB以内をchecked検査する。

[実行例](../../examples/ethernet/dynamic/unicast.ini)、[独立fixture](../verification/fixtures/network-extensions/dynamic/expected.json)、[製品検証記録](../verification/results/network-extension-product-2026-10-07.json) を備える。試験ごとの実施範囲と未照合組合せは [検証仕様](../verification/cases/Ethernet動的制御検証仕様書.md) に示す。

### 制御ID・集計の実装補足

`DynamicControlRecord.synthetic`は内部のaudit識別に用い、JSONへ出さない。入力control IDの`@`接頭辞は予約しない。ユーザーIDをそのまま保存し、合成audit IDは全入力IDと衝突しない値を選ぶ。treeのDijkstra走査はsettled neighborを加算前に除き、u64内の有効な最短距離で往復edgeによる不要なoverflowを起こさない。

追加summaryはcommit済みauditからlearn/move/expire/capacity、changed/no_op/stale、理由別filter/copy dropを数え、停止時のpolicy epochを保存する。失敗callbackや未発火controlを含めない。
