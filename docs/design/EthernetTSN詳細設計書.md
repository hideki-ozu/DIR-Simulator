# EthernetTSN詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 2f1e60b`
設計日：`2026-10-07`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）
文書ID：`design-ethernet-tsn`
文書状態：未公開。抽象モデルを実装済みで、native CLI・schema2出力・Viewerに対応する。実施した製品試験と未照合の組合せは検証仕様の実施記録に区別して示す。規格全体適合の証明ではない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | 初回pushに向け、TAS・CBS・PSFPの状態・算術・共有coordinator・確定境界・観測の詳細設計と実装・検証範囲を確定 |

## 1. 統合と所有境界

[TSN仕様](../specs/models/EthernetTSN詳細機能仕様書.md)を動作の正本とする。[複数モデル統合](複数モデル統合詳細設計書.md)と既存`crates/dir-simulator/src/registry/mod.rs`のRegistry/Schema/Envelope/Contextへ接続する。dynamicのtopology・port/FDB・queue・wire codecを所有する同一Ethernet runtimeの拡張として実装し、TSN専用の並行L2実装は作らない。TSN profile factoryからprepared base dynamic stateとTSN stateを一つのcoordinatorへ渡す。旧Ethernet profileの専用dispatchは維持する。

<a id="scheduling"></a>

## 2. prepare、TAS、CBS

```trace
{"id":"design-ethernet-tsn#scheduling","stage":"design","requirements":["DIR-REQ-0248","DIR-REQ-0249","DIR-REQ-0250","DIR-REQ-0251"],"upstream":["architecture#arch-ethernet-tsn"],"state":"confirmed","pending":[]}
```

### 2.1. 内部型

```text
PreparedTsn { outputs:Vec<TsnOutput>, port_index:BTreeMap<String,usize>,
  streams:Vec<StreamConfig>, stream_index:BTreeMap<StreamKey,usize>, updates:Vec<GclUpdate> }
Schedule { id, base_ps:u64, cycle_ps:u64, entries:Vec<GateEntry>,
  prefix_ps:Vec<u64>, class_open_runs:[Vec<OpenRun>;8], open_total_ps:[u64;8] }
Credit { negative:bool, magnitude:u128 } // zeroはnegative=falseに正規化
TsnState { prepared, ports:Vec<PortState>, meters:Vec<Option<MeterState>>, update_cursor }
PortState { oper, schedule_generation, wake_generation, next_wake,
  last_ps, gate_mask, sending, cbs:[Option<CbsState>;8] }
TsnDelta { changed_ports, changed_meters, update_cursor, records, selection, psfp, ... }
NetworkDelta { queue/state差分, dynamic:Option<DynamicDelta>, tsn:TsnDelta, ... }
```

creditはi128へcastせず符号＋magnitudeとする。加減算・比較・0正規化・clampを小さな共通関数へ集約する。入力の`hi*Q,lo*Q,burst*8*Q`、全GCL prefix、bitrate、参照整合、全将来updateとsource正規順をprepareでchecked計算する。積`u64_rate*u64_dt`はu128内でも、既存creditとの和はoverflowし得るため加算前にcapとの差と比較して飽和する。`ceil(n/d)=n/d + (n%d!=0)`でn+d-1を避ける。SOF+duration、候補base+k*cycle、連番/世代incrementはchecked。timerの候補がu64を越えれば予定候補をu128でsnapshotに保持し実予約なし、実送信のEOF/releaseが越える場合はE-0004として送信開始をcommitしない。

prepareはruntimeへ未知キーを持ち込まず、全outputと8class、CBS重複、GCL duration合計、dynamic profile組合せ、meter・filter参照を検証する。schedule IDは各portの初期/更新を通して一意、更新IDは全実行一意。runtimeから新たなconfigを受理するAPIは初版にはない。

### 2.2. 半開区間とwindow計算

prefixのupper_boundで `(t-base)%cycle` を探索する。ちょうどprefix境界は次entry。classごとに隣接openを結合し、周期末と先頭のopenも循環結合する。all-openは無限上限、all-closedはrunなし。`next_fit(port,class,t,occupancy)`は現在run残長→次の十分長いrunを高々entry数だけ探索し、次pending GCL effectiveで切断する。未来base/updateがあればその時刻で再評価、現在周期にfit runが無ければnever_eligibleとする。候補探索で周期を逐一進めない。

### 2.3. handlerと予約

| handler | 処理 |
| --- | --- |
| phase0 coordinator(t) | affected creditを旧modeでtまで積分。wire完了/release、link/STP、registration/membership、aging、GCL切替のbatchを作る。状態変更後のmodeを導出、dirty portを集約 |
| arrival(t) phase1 | ingress分類とPSFP、処理予約/offer。queue変化前にcredit積分、変化後modeを再導出 |
| arbitrate(t) phase2 | dirty port辞書順、最新policyで不適格copyをdrop。全class先頭のgate/fit/C>=0を評価、最大priorityを一つ選択。SOF/EOF/release/arrival予約と容量減算をpreflight後commit |
| wake(t) phase0 | 世代一致だけcoordinatorのdirtyへ登録。不一致timerは意味的no-opでmodel_records/metricsを出さない |
| GCL update(t) phase0内 | update ID順にoperを交換しschedule_generation++, wake_generation++。旧timerはcancel、gate mode再導出、dirty化 |

`Context::send_request_at`はphase1の入力通知だけに使用し、phase0内部timerは`Context::schedule_at`で予約する。Engineのglobal phase順序を増設せずprofile-owned coordinatorが同時刻のwire/controlを一つのbatchに集約する。他profileのcallback順は変更しない。batchから過去phaseへの同時刻通知が必要な場合は既存delta規則を使い、TSN内部の通常経路はそれを必要としない。

CBS advanceは遅延評価で省略されたgate境界を無視してはならない。自classSendingでは全dtを積分する。非Sendingではその区間のscheduleから`open_elapsed(class,t0,t)`を求め、idle_slopeとの積だけを加算する。open_elapsedはbase前を除外し、周期のopen合計×完全周期数＋先頭/末尾のprefix差で求め、cycle逐次走査はしない。GCL交換で必ず一度advanceするため区間内のscheduleは一つ、queue/backlogの変化でも必ずadvanceする。空かつ負creditなら0で飽和、backlogありならhiで飽和し、空かつ正creditなら即0 reset。記録のmode/slopeはその時点のgate値であり、次recordまでgate不変とは解釈しない。release境界はSendingを終了してからempty/backlog/gateを評価する。gate reopen時は凍結終了、その時点の負creditから再計算する。credit zero wakeは`now+ceil(abs(C)/idle)`と次gate閉鎖/更新の最小を予約し、gate変更/先頭変更/releaseでwake_generationを上げて旧wakeをcancelする。0遅延wakeを反復せず同batch dirtyへ畳み込む。

各portは意味上有効な次wakeを最大一つ保持する。busyならreleaseを候補とし、base前ならbase、closedなら次open、負creditなら0到達または先行gate変更、guardなら次fit、更新が先ならeffectiveを選ぶ。全classの必要候補とcoordinatorの管理操作から最小を取る。queueが空でも負credit/openなら0回復に有限一回のwakeを許す。空/credit0、全closed、never_eligibleで将来更新なしならwakeなし。gate履歴を得るためだけの無限周期tickは作らず、gate recordは関連batchで評価した状態、metadata scheduleからquiet区間の表示を導出する。未使用gate境界を含む全時刻イベント列は出力しない。

### 2.4. callbackの原子性

`TsnState` はimmutable committed stateと同batch `TsnDelta` のviewから必要port/meter差分だけを計画する。`runtime/network.rs` の `NetworkDelta` がqueue・dynamic policy・TSN token/credit・recordsをまとめ、`runtime/registered.rs` の `NetworkArena::prepare` と `NetworkState::reserve` が全Effectsと必要容量を事前検査する。`PreparedNetworkEffects` と完成state bufferの適用以降は失敗点を残さず確定する。公開Model traitのsignatureは変更しない。通常dropは成功callbackの効果であり、算術/Effectsエラーではcallback全体を未公開とする。全state cloneを通常経路に置かない。

<a id="policing"></a>

## 3. PSFP、snapshot、実装順

```trace
{"id":"design-ethernet-tsn#policing","stage":"design","requirements":["DIR-REQ-0252","DIR-REQ-0253","DIR-REQ-0254","DIR-REQ-0255"],"upstream":["architecture#arch-ethernet-tsn"],"state":"confirmed","pending":[]}
```

### 3.1. PSFP差分

`StreamKey=(IngressId,Mac48,Vid,Priority)`のBTreeMapで一件取得する。`MeterState { committed:u128, peak:u128, last_evaluated_ps:u64 }`はstreamごとに所有し、上限はbyte→bit*Q変換済み。meter評価時に `refill=min(cap-old, rate*dt)` を足すのでcap加算overflowを避ける。時刻逆行は不変条件エラー。判定はpeak不足→committed不足→green順、yellow dropでもpeak消費をcommitする。SDU/gateでdropした際はmeter state未変更。

`TsnState::plan_arrival_in` はimmutable incoming wireのMとstaged meter viewを使い、`TsnDelta` に一回のpolicing結果とbucket差分を準備する。受信IDを既存receptionと共用し、egress複製はdecisionを参照するだけ。drop reasonはpsfp_max_sdu/psfp_gate_closed/psfp_meter_red/psfp_meter_yellow、通過はpass、未一致はbypassを記録する。未一致はstream_id=nullを許す。meter無効・前段dropはbucket/nullとcolor/null、消費0。

dynamicとの内部interfaceは`PolicySnapshot { policy_epoch, topology_generation, roles, link_up, effective_vlans }`を返す` snapshot_policy()`、`eligible_egress(port,vid,snapshot)->Eligible|Ineligible(LinkDown|StpDiscarding|VlanUnregistered)`、`policy_epoch()->u64`とする。snapshotはbatch中immutableで、同一epochのpolicyを全候補に用いる。dynamic policy_epoch変更はport dirtyを立てる。arbitration時に既存dynamic eligibility関数へcopyと最新epochを渡し、不適格なら理由付きcopy dropとqueue byte減算、empty credit正値resetを同deltaへ入れる。再検査はlink/STP/VIDだけとし、offer時に固定した目的port集合をgroup/source membership・FDB変更で遡及取消・追加しない。policing countersとtokenは触らない。PSFPを通過してqueue_fullとなったframeはpass1/drop(copy)1であり、二重PSFP dropにしない。

### 3.2. 出力と純粋replay

native snapshotはu64時刻/u128数値のまま保持し、JSON projectionで正規十進文字列へ変換する。Registryへdynamicのframe/transfer/reception/control/policyとTSNのgate/credit/policing/decisionの計9種のSchema(name,1)を宣言し、metadataにも全集合を保存する。dynamicのvisit ID・epoch・親子lineageを継承し、旧VLANの`@fromport`形式識別子や旧schemaのfield追加で代用しない。credit観測は積分後の値と次区間のslope、gate・queue変化によりmodeが変わる時刻に出す。viewerはmetadataのscheduleと最後のcredit recordから同じ整数clamp/resetを評価し、描画位置比だけNumberへ変換する。meterは次評価まで最後の実績と潜在refill値を区別し、未評価の消費を作らない。

partial snapshotのHは共通停止契約に従い、[0,H)実績とfuture planを分離する。queue・current transfer・credit(t=Hの左極限)・bucketと最終評価時刻・oper schedule/generation・future timerを保存する。H境界の不実行callbackを適用してはならない。viewer stepはrecord順(effect_seqを含む)でpure replayし、同じ到達先へ戻れば同じstateを構築する。

### 3.3. 実装配置と検証

| 実装ファイル（crate src基準） | 内容 |
| --- | --- |
| `input/ethernet/tsn.rs`、`lib/types/ethernet/tsn.rs`、`input/network.rs`、`registry/mod.rs` | wrapper/型/参照/checked prepare。dynamic baseは `PreparedNetwork.dynamic` に独立保持 |
| `runtime/ethernet/tsn.rs` | signed credit、TAS window、meterと純粋 `TsnDelta` |
| `runtime/network.rs`、`runtime/registered.rs`、`runtime/ethernet/l2.rs` | phase0 batch、phase2 selection/SOF、期限世代、Effects/stateの確定境界と共有L2 |
| `lib/snapshot/ethernet/tsn.rs`、`output/network.rs` | 4種TSN DTO、schema2 projection・metadata・partial・集計 |
| `tool/viewer/assets/ethernet-model.js`、`ethernet-app.js` | BigInt gate/credit/PSFP replayとstrict schema |
| `tests/network_extensions.rs`、各module単体試験、root `tests/network_viewer_model.test.cjs` | 独立vectorとCLI実測、input/planner/Viewer検証 |
| `examples/ethernet/tsn/`、`docs/verification/fixtures/network-extensions/tsn/expected.json`（repo基準） | 完全入力例と製品serializerから独立した期待値 |

[実行例](../../examples/ethernet/tsn/tas.ini)、[製品検証記録](../verification/results/network-extension-product-2026-10-07.json) と [検証仕様](../verification/cases/EthernetTSN検証仕様書.md) を参照する。設計算術の照合と、named testで実行した製品経路を区別し、全列挙組合せの合格やIEEE conformanceを意味する記録にしない。

### 集計値の実装補足

commit済みpolicing/decision auditから色・破棄・gate/guard/credit阻止の観測件数を集計する。`ethernet.tsn.gate_open`はgate auditで観測した開class数の総和、`ethernet.tsn.credit`は停止snapshotのclass別creditをbit値へ一度変換した表示用numberである。厳密な符号・u128 magnitude・Qはmodel_recordsとsnapshotに保存し、runtimeの計算には浮動小数点を使わない。初期policy・gateと空queueのready decisionは初期状態記録としてT=0でも保存する。
