# CAN・Ethernet変換詳細設計書

文書バージョン：`1.1.0`
設計日：`2026-10-07`
対象GitHubバージョン：`main @ 2f1e60b`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）
文書ID：`design-can-ethernet`
文書状態：未公開。抽象モデルを実装済みで、native CLI・schema2出力・Viewerに対応する。実施した製品試験と未照合の組合せは検証仕様の実施記録に区別して示す。規格全体適合の証明ではない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | 初回pushに向け、CAN↔Ethernet変換のprepare・codec・協調実行・lineage・結果の詳細設計と実装・検証範囲を確定 |

[詳細機能仕様](../specs/models/CAN・Ethernet変換詳細機能仕様書.md)を入出力の正本とする。既存[GW](GWモデル詳細設計書.md)、[VLAN](EthernetVLAN・マルチキャスト詳細設計書.md)、[複数モデル統合](複数モデル統合詳細設計書.md)のprofileを並列runして結果を合成する方式は採用しない。一つの共通Engineと一つのcomposite coordinatorで媒体間因果を処理する。

<a id="codec"></a>

## 1. prepare・登録・codec

```trace
{"id":"design-can-ethernet#codec","stage":"design","requirements":["DIR-REQ-0230","DIR-REQ-0231","DIR-REQ-0232","DIR-REQ-0233"],"upstream":["architecture#arch-can-ethernet"],"state":"confirmed","pending":[]}
```

| 実装ファイル | 責務 |
| --- | --- |
| `crates/dir-simulator/src/input/can_ethernet.rs`、`input/network/topology.rs` | 厳密JSON、混合NED解決、所有/参照検査、canonical prepare |
| `crates/dir-simulator/src/lib/types/can_ethernet.rs` | immutable設定、媒体変換規則、枝別lineage |
| `crates/dir-simulator/src/runtime/can_ethernet/protocol.rs` | 11+DLC byteのDIRC codec、offset付き拒否理由 |
| `crates/dir-simulator/src/runtime/can_ethernet.rs` | `BridgeState`の純粋plan、CAN仲裁、RX保持、conversion/branch/segment差分 |
| `crates/dir-simulator/src/runtime/network.rs`、`runtime/registered.rs` | 一つのnetwork coordinator、共通Engine/FES、staged確定境界 |
| `crates/dir-simulator/src/runtime/ethernet/l2.rs` | native Ethernetと共用するserializer・分類・wire/容量算術 |
| `crates/dir-simulator/src/lib/snapshot/can_ethernet.rs`、`output/network.rs` | typed record、schema2 projection、媒体別summary・RX/終端母数 |
| `crates/dir-simulator/tests/network_extensions.rs`、各新moduleの単体試験 | 製品CLI、独立期待値との照合、planner/codec/input試験 |
| `docs/verification/fixtures/network-extensions/can-ethernet/` | 製品から独立したR01/R02/R03・RX期待値 |
| `examples/can-ethernet/` | 双方向、二Bus fanout、有限RXの完全INI/NED/model/workload |
| `crates/dir-simulator/src/tool/viewer/assets/ethernet-model.js`、`ethernet-app.js` | 混合媒体validator、lineage選択、pure replay |

`Registry::new()`は `can.ethernet.gateway.v1` とprivate module `dir.network.Coordinator` を登録する。prepareは `PreparedRegistered.network` に `PreparedNetwork` を保存し、coordinatorのID・subjectは `@network` とする。Gateway実体pathはrecord.data.gatewayに保存する。混合NEDは既存CAN/Ethernet resolverを再利用し、runtime child自身はFESやrun loopを持たない。native CAN bit serializerとEthernet L2純粋関数を共用する。公開Model traitの署名を変更せず、通常の外部モデル呼出しとprivate network staged経路を内部で分ける。

| 内部型 | 不変条件 |
| --- | --- |
| `PreparedCanEthernet` | resolved topology、既存CAN/VLAN設定、gateways:Vec<BridgeGateway>とsort済rules、generatorの媒体別設定・所有検査 |
| `DirCanPacket` | format:Standard/Extended、id:u32（standard≤2047、extended≤536870911）、data:Vec<u8>、長さ≤8。encode結果は常に11+data.len() |
| `BridgeLineage` | OriginId、親record ID、ordered Gateway訪問列。外部wire decoderから生成しない |
| `Conversion` / `ConversionRecord` | ingress media record、受理/ready/release、RX slot、各BranchState、reason |
| `Branch` / `BranchRecord` | egress handle、媒体wire、offer時刻、Waiting/Admitted/Dropped。Admittedから再offer不可 |
| `BridgeDelta` / `NetworkDelta` | touched state/queue差分、records、予約、checked next IDs。先行差分を読む同batch view |

prepare順は厳密JSON→共通NED→CAN所属/eth tree→Gateway所有集合→rule exact keyと全egress→priority/VLAN membership→workload codec候補→canonical ID予約。空負荷でも全tableを検査する。PCPはto_eth指定値をsource classとtag両方へ設定し、untaggedではsource classのみへ設定する。次hopの受信classは当該port policyから再分類するためto_can照合は実ingress classを用い、元PCPを復元しない。

encodeは`Vec::try_reserve_exact(11+len)`成功後にmagic/version/flags/ID/DLC/dataを置く。decodeは先に11byte最小長、DLC、11+DLC checked値、予約flags bit1..7=0とpaddingを確認してからsliceを取得する。flags bit0からformatを復元し、standardではID≤0x7ff、extendedではID≤0x1fffffffを確認する。rule検索keyにはformatを含め、出力format/IDと入力format/IDを別fieldに保存する。CANとEthernetのCRCは媒体serializerだけが担当する。PCP/VIDはinner payloadへ入れない。

<a id="runtime"></a>

## 2. 単一FES、commit、出力

```trace
{"id":"design-can-ethernet#runtime","stage":"design","requirements":["DIR-REQ-0230","DIR-REQ-0234","DIR-REQ-0235","DIR-REQ-0236","DIR-REQ-0237","DIR-REQ-0238","DIR-REQ-0239"],"upstream":["architecture#arch-can-ethernet"],"state":"confirmed","pending":[]}
```

### 2.1. 共通APIとの境界

共通Engineは `(time_ps,delta,phase,seq)` の単一FESを所有する。登録eventは `dir.network.control/1`（phase0）と `dir.network.event/1`（phase1）。private `Event` と `BridgeTimer` は内部serde codecで運ばれ、Gateway自身から公開eventを送る別run loopはない。wire codec DIRCとは別である。completion/controlはcoordinatorのdue集合でまとめて処理し、媒体resourceは共通phase2 arbitrationを使う。

coordinatorは時刻tの内部batchで既知completion/resource release/controlを先に処理し、その後phase1 arrival/offerを処理する。物理EOF到達はphase0 timer→同tのphase1 arrival。全枝の受理候補がそろってからresourceをdirtyにする。`request_arbitration`の共通sealを利用し、全媒体を走査する別イベントループは作らない。共通Engineのtime/delta/phase/seq規則は変えず、phase2から同t phase1への効果は次deltaへ送る。0delay callbackから過去phase0を新規作成せず、処理完了を現在batch内で畳み込む。

CAN SOFで待機queueをdequeue、EOFで受信予定、intermission完了でbus再利用。Ethernet SOFでqueueをdequeue、EOFでarrival予約、IFGを含むreleaseで方向再利用。CAN/Ethernet同時SOFは独立resourceとして許す。送信中とqueue待機を分け、TX容量解放と媒体解放を同一時刻へ丸めない。

### 2.2. batch手順と失敗復元

1. committed stateを読んでingress検査、RX入場、rule/lineage、全egressを決定。canonical順にchildを構築する。
2. 各branchのwire、class、MAC bytes/件数、ready/offer/SOF以降の予約時刻、event/record ID、metric増分をchecked演算で計画する。u128中間値をu64へ変換する前に範囲確認する。
3. 同じbatch内の先行admissionを容量計算へ含め、最終queue差分と待機list差分を作る。満杯はWaiting、容量0/過大frameはDroppedとして通常結果にする。
4. `try_reserve`で所有Vecの必要容量を先取りし、effect schema/record validator、予約event数、seq、metric範囲をすべて検査する。一枝の内部失敗なら全NetworkDeltaとEffectBatchを破棄する。
5. 共通Engineのeffect commitとcoordinator state delta適用を一つの成功境界へ統合する。journal、FES、event IDs、queue、RX/枝状態を一緒に進める。commit後にだけ現イベントを消費する。

`runtime/registered.rs` のprivate network経路は、committed `NetworkState` を読むplannerから `NetworkDelta` とEffectBatchを受け取る。`NetworkArena::prepare` が参照・時刻・ID・必要容量を検査して `PreparedNetworkEffects` を完成させ、`NetworkState::reserve` による必要buffer予約も済ませる。以後のEngine効果とstate delta適用は所有値の移動・交換で確定する。公開Model traitへclone/rollbackを要求しない。回復できる `try_reserve` 失敗は診断し、allocatorのprocess abortは部分結果保証の対象外とする。

再試行は容量を解放したegressに対する未受理の枝だけをready順で検査する。枝ごとのadmitted状態と同一batchの容量差分により二重予約を防ぐ。最後の枝がAdmitted/Droppedへ変わるcallbackでRXを解放し、解放時刻と理由を一回記録する。既に送信中の枝があってもRXは未受理枝のため保持する。callbackエラー時のRX/queue/journalは前の成功prefixのまま、失敗イベントはpendingに残る。finishから補完変換や予定受信を生成しない。

### 2.3. schema2、metrics、viewer

output_schema_version=2、共通5ファイルとmanifest-last公開を再利用する。固有record Schema.nameは`dir.can_ethernet.conversion`/`dir.can_ethernet.branch`、および`dir.can_ethernet.segment`、すべてversion=1。既存CAN/VLAN recordの意味を変えず、関連付けは新recordの参照列に置く。prefixが参照するorigin/parent/childは同batch又は既存成功journalに必ず存在する。まだ作成されないchildはnullとしplanned_child_idを別列へ置く。

conversion/branchのupsertは、直前のstaged又はcommitted DTOと比較し、新規行・変更行だけを発行する。子branchのSOFだけが更新された場合は当該branchを発行し、conversionと兄弟branchの包絡時刻を進めない。容量が変わらない再試行でも行を再発行しない。内部差分は同じ確定境界で適用し、conversionの包絡時刻は自身のobserved/ready/released実績の最大値と一致させる。

共通summaryにはprofile・termination・committed eventsを、媒体別summaryには分母/成功数、busyを保存する。conversion拒否は理由別に一回、RX maxはcommit後の占有から更新する。partial出力の比率は実SOFで確定した母数を使い、未受信を成功へ数えない。byte量は媒体別wireで集計する。end-to-endは実SOFごとの `(origin_id,segment_id,branch_lineage,terminal_id)` 対象集合を凍結し、Gateway以外のapplication受信者だけを分母へ入れる。CANはbus送信一個、Ethernetはframeの最初のsource送信一個をsegmentとし、Switch hopは継承する。native元送信にもsegment IDを発行する。`SegmentRecord`は正規順の `Vec<SegmentTarget>` と固定target_countを持ち、SOFでsegment recordをcommitする。以後の受信処理完了callbackでは対象tupleの未完了→完了だけを許し、重複通知は分子/遅延を再加算しない。SOF前のbranchや未発生の変換先からtargetを作らない。segment recordの全tuple/実completed_ps/reception参照から `sum(completed)/sum(target_count)` を再構築する。遅延はtupleごとにorigin生成から受信処理完了までの差分で求め、hop加算をしない。

viewer loaderはprofile/schema集合、参照閉包、codec長、PCPとtag/classの整合、訪問列重複、branch状態遷移を検査。CAN既存modelとEthernet既存modelへ投影してからGatewayの関係を描く。観測済みだけでslider状態を復元し、T停止のplanned EOFからreceiverを作らない。媒体受信と変換成功は別欄、途中枝を含むorigin選択を用意する。

## 3. 実装順序と完了条件

| 段階 | 作業 | 合格条件 |
| --- | --- | --- |
| 1 | codec/prepare/登録、無効profile拒否 | codec固定vector、全負例、未知field/重複所有拒否 |
| 2 | 純関数抽出と単一FES adapter | 既存13 profile fixtureの結果不変、新媒体同時SOF |
| 3 | prepared-callback commit境界 | 各preflight地点失敗注入でstate/journal/FES不変 |
| 4 | 双方向・lineage・有限容量 | store-forward数値、fanout待機・循環停止 |
| 5 | schema2/metrics/viewer | 参照閉包、保存則、T±1ps、前後再生一致 |
| 6 | examples/公開docs/回帰 | fmt、clippy、locked tests、Node/ブラウザ、strict trace |

製品試験のcommand・source/fixture hash・成果物と実施範囲は [製品検証記録](../verification/results/network-extension-product-2026-10-07.json) に記録する。段階表は完了条件の全組合せが試験済みという主張ではなく、named testと残る照合範囲は [検証仕様](../verification/cases/CAN・Ethernet変換検証仕様書.md) を参照する。設計時の独立計算や既存profileの成功を、新profileの製品合格へ転用しない。

`BridgeState::plan_in_delta` と `plan_can_arbitration_in_delta` は同一 `BridgeDelta` の先行変更を参照できる。`NetworkDelta` がEthernet admission差分と合わせ、全state cloneを行わず変更対象partitionとbufferを完成させて確定境界へ渡す。CAN waiting queueのPoint履歴もこの境界で公開し、on-wire requestをqueue占有へ数えない。

### 媒体間IDと受信母数の実装補足

`BridgeLineage::native` は `can:<request ID>` / `ethernet:<frame ID>` をroot originおよびnative segment IDに用いる。既存native record IDsは保持し、同じgenerator名・ordinalでもschemaと修飾originで区別する。`output/network.rs`はoriginの媒体を先に選び、その媒体の生成時刻からterminal latencyを計算する。Viewerもschema-qualified indexを使う。

Ethernet segmentのSOF母数は元VIDとindividual/broadcast/groupの宛先member集合で固定する。下流のadmit拒否やFDBによる誤経路で対象を減らさず、当該tupleを未完了として残す。送信開始で共有Port容量が空いた通知は全classの待機conversionへ届き、ready/origin/conversion/egress順で再offerする。
