# EthernetTSN検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 2f1e60b`
設計日：`2026-10-07`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）
文書ID：`verification-ethernet-tsn`
文書状態：未公開。抽象モデルを実装済みで、native CLI・schema2出力・Viewerに対応する。実施した製品試験と未照合の組合せは検証仕様の実施記録に区別して示す。規格全体適合の証明ではない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | 初回pushに向け、TAS・CBS・PSFPの独立算術・合成・停止・部分結果の検証契約と製品照合範囲を規定 |

## 1. 証拠の境界

以下は[TSN仕様](../../specs/models/EthernetTSN詳細機能仕様書.md)・[設計](../../design/EthernetTSN詳細設計書.md)の独立期待値と受入契約である。runtime・CLI・schema2・Viewerを実装し、製品fixtureとnamed testの実施範囲を第6節に示す。下表の全subcase・全合成を試験済みとは扱わない。算術を紙上/独立整数計算で照合してもIEEE conformanceや製品成功を意味しない。1Gbps、Q=10^12、処理遅延0、伝搬遅延0を明記しないcaseの共通条件とする。

設計段階の[独立算術vector](../fixtures/network-extensions/design-vectors.json)と[文書・設計検証記録](../results/network-extension-design-2026-10-07.json)を参照する。これらはcodec・時刻・credit・token・受信母数の期待値と文書整合性の証拠であり、製品用入力fixtureの実行結果ではない。

<a id="dir-test-0113"></a>

## 2. DIR-TEST-0113：入力・TAS・atomic schedule

```trace
{"id":"DIR-TEST-0113","stage":"verification","requirements":["DIR-REQ-0248","DIR-REQ-0249","DIR-REQ-0250","DIR-REQ-0251"],"upstream":["design-ethernet-tsn#scheduling"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0063。

| case | 入力 | 独立期待値 |
| --- | --- | --- |
| TAS-01 | untagged M=64、class7 open [0,1000000)、周期2000000、offer=0 | SOF0、EOF576000、release672000 |
| TAS-02 | 同じframe offer328000 / 328001 | 前者release1000000で許可、後者次SOF2000000 |
| TAS-03 | tagged M=68、offer296000 / 296001 | occupancy704000、前者release1000000、後者次SOF2000000 |
| TAS-04 | 隣接[0,400000),[400000,1000000)が共にclass7 open | 結合幅1000000、SOF0を許可 |
| TAS-05 | cycle1000000、open[0,400000)と[700000,1000000)、offer700000 | 周期跨ぎ窓終端1400000、release1372000で許可 |
| TAS-06 | base1000000、offer0 | no_active_schedule、wake1000000、SOF1000000 |
| TAS-07 | 全closed又は最大open幅600000、M64、更新なし | queue保持never_eligible、送信0、無限tickなし |
| TAS-08 | 仕様JSONのu1、offer1900000のclass7 | effective2000000で新g1、新窓[2000000,2800000)、SOF2000000/release2672000 |
| TAS-09 | 旧常時open、update effective500000、M64 offer0 | 更新境界で旧窓を切断、SOF0不可、新GCLがopenならSOF500000 |
| TAS-10 | updateとrelease/gate境界同時刻、旧wakeも残存 | completion→update→phase2、新generationだけ有効、二重送信0 |

負例はcycle=0、duration=0/合計不一致、priority重複/8/bool、未知キー/重複JSONキー、base/effective不一致、submitted>effective、同port/effective重複、未接続port、半二重/PAUSE、CBS slope0/R以上、u64非正規文字列を一つずつ注入する。全件prepare失敗・callback0。submitted=effectiveは受理。空負荷・T以後の不正GCLも拒否。配列順とJSONキー順を反転してcanonical実績一致。

<a id="dir-test-0114"></a>

## 3. DIR-TEST-0114：CBS・容量・wake

```trace
{"id":"DIR-TEST-0114","stage":"verification","requirements":["DIR-REQ-0248","DIR-REQ-0249","DIR-REQ-0250","DIR-REQ-0251"],"upstream":["design-ethernet-tsn#scheduling"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0063。hi=lo=1000bit、idle=250000000bit/s、M64、backlog2、gate常時open。

| 時刻ps | 状態/独立credit(bit) | 期待動作 |
| --- | --- | --- |
| 0 | 0 | 第1frame SOF |
| 576000 | -432 | EOF、IFG中もSending |
| 672000 | -504 | release、次frameはcredit_negative |
| 2688000 | 0 (`-504+0.00025*2016000`) | 第2frame SOF |
| 3360000 | -504 | 第2release、空でも負credit回復 |
| 5376000 | 0 | empty zero、これ以後credit tickなし |

gateが1000000で閉じ、2000000で再openの場合、1000000のC=-422bit、closed区間は凍結、0回復は3688000ps、次SOF3688000。その前の旧wake2688000は世代不一致で何も変更しない。このcaseの初回openは[0,1000000)、次openは[2000000,5000000)とする。

待機classが他class送信で1000000ps待つ場合+250bit、hi=100bitなら+100で飽和。全待機copyのdynamic dropで空になれば正credit即0。lo=100bitなら最初のrelease=-100、0回復1072000ps。最小端数caseはC numerator=-1,idle=3でwake1ps、増分3後0以上（空なら0）、切捨て0ps wakeを禁止する。credit bound=0の明示入力も飽和0として受理する。

port/class容量64byteへ同時刻M64を2個offerすればphase1で1受理/1queue_full、phase2が同時刻入場を先取りしない。tagged M68はbyte上限64へdrop。高class guard不可/負creditならeligible低classを選択する。class内先頭が大きくfitしなくても小さい後続の追越なし。timer世代overflow、EOF/release加算overflowを注入しE-0004・callback差分未commitを確認する。u64上限外の将来wakeは予約せず候補値保持。

<a id="dir-test-0115"></a>

## 4. DIR-TEST-0115：PSFP filter・gate・meter

```trace
{"id":"DIR-TEST-0115","stage":"verification","requirements":["DIR-REQ-0252","DIR-REQ-0253","DIR-REQ-0254","DIR-REQ-0255"],"upstream":["design-ethernet-tsn#policing"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0064。M64byte、committed burst64byte、peak burst128byte、committed=8000000bit/s、peak=16000000bit/s、yellow_action=pass。表のtokenはbyte相当で記述し実装比較は各値*8*Qの整数とする。

| 到着ps | refill後(C,P) | 色/消費後(C,P) | 期待 |
| --- | --- | --- | --- |
| 0（到着ID a） | (64,128) | green/(0,64) | pass |
| 0（到着ID b） | (0,64) | yellow/(0,0) | pass、同時刻順固定 |
| 0（到着ID c） | (0,0) | red/(0,0) | drop |
| 32000000 | (32,64) | yellow/(32,0) | pass |
| 96000000 | (64,128) | green/(0,64) | pass |

yellow_action=dropならbはdropだがpeak消費は同じ。Mがpeak_burst_bytesを越える場合を受理設定とし、そのframeはred（設定失敗ではない）。committed_burst_bytes<M<=peak_burst_bytesならpeak tokenが足りる状態ではyellowとなり得る。rate*dtが大きいrefillはcapへ飽和しoverflowしない。token端数（rate=1bit/s,dt=1ps）ではnumeratorを1増やし丸めない。

filterのmax_sdu=64でM64受理、M68はmeter非評価でpsfp_max_sdu。gate open[0,1000000)の到着999999受理、1000000はpsfp_gate_closed。gate前段drop後のrefillは最終meter評価時刻から行う。VID/PCP/ingress/dst一つだけ違うframeはbypass、同tuple rule重複・未知ingress・burst0・committed>peakはprepare拒否。untaggedの分類は受信Port PVID/default_priorityを用い送信元flow情報を使わない。

一つの受信を3egressへ複製してもgreen消費64byte一回。一つqueue_fullでもrefund0、残るcopyは送信可。同frameの別hop到着ではそのhopのmeterを一回評価する。meter消費後のEffects予約失敗は全差分未commitとし、正常な後段dropとは区別する。

<a id="dir-test-0116"></a>

## 5. DIR-TEST-0116：dynamic合成・partial・viewer・互換

```trace
{"id":"DIR-TEST-0116","stage":"verification","requirements":["DIR-REQ-0252","DIR-REQ-0253","DIR-REQ-0254","DIR-REQ-0255"],"upstream":["design-ethernet-tsn#policing"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0064。

1. 同tのwire completion、link down/STP、registration、aging、GCL update、arrival、arbitrationを一fixtureに置き、規定群順と同種source正規順のevent/effect_seqを比較する。arrivalは更新後gate・policyを使う。
2. PSFP通過済みqueued copyのegress VID登録を失効させる。最新epochでvlan_unregistered drop、queue bytes減算、token refund0、送信credit減算0。group/source membershipやFDBだけを変更した別caseでは既存copyを取消せず、offer済み目的port集合を維持する。同tで正creditかつemptyなら0 reset。onwire copyは非中断でEOFとarrivalを保持し受信側の到着時policyで判定する。
3. T=576000はEOF実績なし、T=672000はEOF実績あり/release未実行、T=2688000はCBS 0到達wake未実行/第2SOFなし。planned timestampを完了件数・receiverへ計上しない。T1/T2までの実績prefixが一致する。
4. JSON/CSV/5ファイルmanifest整合、dynamic 5種＋TSN 4種の全Schema(name,1)とmetadata集合、visit IDとepoch、metric units、negative credit符号とu128文字列、record参照先、policing色別件数を照合。metadataだけのquiet gate表示と観測イベントを混同しない。
5. viewerでgate変更・負credit/0回復・yellow/red・blocked reasonを確認。時刻>2^53、desktop/mobile、step forward/backward同一到達先一致、連続再生線強調、partial未到達receiverなしをbrowser試験する。
6. 旧L2/QoS/VLAN/媒体/dynamic/CANの既存fixtureを同buildで回帰し、既存record/schema/時刻不変と旧profileのTSNキー拒否を確認する。TSN全機能無効（tas=null,cbs=[],streams=[]）では同じdynamic入力の転送実績と一致させる（TSN metadata/schema固有差だけ許容）。

## 6. 実施記録と照合範囲

[製品検証記録](../results/network-extension-product-2026-10-07.json) は入力・独立expected・製品actual・source/toolchain・実行command・結果を保存する。[独立製品fixture](../fixtures/network-extensions/tsn/expected.json) と `every_independent_design_vector_runs_against_production_planners` は、期待値とproduction plannerを比較する。設計記録と `design-vectors.json` の `product_execution:false` を遡って変更しない。Rust/Nodeの製品試験と設計算術の成功を別に記録する。

| TEST ID | named test・製品経路 | この実施で照合する範囲 |
| --- | --- | --- |
| DIR-TEST-0113 | input `rejects_missing_unknown_numeric_bool_and_noncanonical_fields` / `validates_every_future_update_and_normalizes_order`、runtime `tas_01_to_05_ifg_exact_fit_adjacent_and_circular` / `tas_06_07_future_base_closed_and_oversized_no_periodic_tick` / `tas_08_09_10_control_clips_all_open_invalidates_old_wake` / `schedule_generation_overflow_and_empty_control_noop_are_atomic`、統合 `tsn_wire_scheduling_matches_independent_tas_and_cbs_boundaries` | TAS-01〜10のproduction window/planner vector、未来update・strict入力、GCL境界世代と原子性、TAS/TAS-updateの実SOF。全JSONキー順反転・全port/媒体組合せまで網羅する意味ではない |
| DIR-TEST-0114 | `cbs_ifg_and_two_frame_independent_timeline` / `cbs_lazy_freeze_and_old_timer_noop` / `cbs_saturation_reset_fractional_recovery_and_full_u128` / `lower_ready_class_can_send_and_fifo_head_never_bypassed` / `overflow_plans_are_atomic_future_wake_u128_and_no_busy_loop`、統合CBS/CBS-gated、共通 `staged_never_eligible_retains_queue_without_infinite_gate_ticks` | IFGまでSending、0回復2688000/3688000ps、freeze/clamp/端数/u128、低class/FIFO、overflow未commit、never_eligible有限wake。全class競合やdynamic dropの各組合せをCLIで実行した主張ではない |
| DIR-TEST-0115 | input `validates_stream_tuple_gate_and_meter`、runtime `psfp_five_arrival_independent_vector_and_yellow_drop` / `psfp_sdu_and_gate_drops_preserve_meter_last_time_and_bypass_tuple` / `psfp_red_refill_saturation_fraction_no_refund_and_discarded_delta` / `composed_arrivals_share_staged_meter_but_never_mutate_before_apply`、統合 `psfp_product_arrivals_consume_once_and_stream_gate_drops_before_meter` | 5到着の色/2bucket、yellow drop、前段SDU/gateでmeter非評価、tuple bypass、飽和/端数/差分破棄、同batch消費、PSFPとstream gateの製品実行。3egress実fanoutとqueue_fullの全組合せは別途必要 |
| DIR-TEST-0116 | `partial_snapshot_keeps_unexecuted_boundary_and_records_decimal` / `composed_control_selection_sof_commits_once_and_stale_wake_stays_empty` / `metadata_roundtrips_and_snapshot_json_stays_decimal` / `staged_return_to_committed_mode_emits_correcting_credit_observation`、共通staged failure試験、Viewer model/browser | 未実行境界/null、同batch制御→selection/SOF、古いwake、metadata/D projection、credit観測とprefix replay、quiet gate、BigInt・前後seek。第5節の全domain同時刻fixture・全T境界・TSN無効とdynamicの全実績同値の網羅を意味しない |

完全入力例は [TAS](../../../examples/ethernet/tsn/tas.ini)、[TAS-update](../../../examples/ethernet/tsn/tas-update.ini)、[CBS](../../../examples/ethernet/tsn/cbs.ini)、[CBS-gated](../../../examples/ethernet/tsn/cbs-gated.ini)、[PSFP](../../../examples/ethernet/tsn/psfp.ini)、[PSFP-gate](../../../examples/ethernet/tsn/psfp-gate.ini)。主な実施commandは `/home/hideki/.cargo/bin/cargo test --locked -p dir-simulator --lib ethernet::tsn`、`cargo test --locked -p dir-simulator --test network_extensions`、`node --test tests/network_viewer_model.test.cjs`、`node tests/network_viewer_browser.cjs <製品results.json> ...`。format/clippy、全旧profile回帰、browser対象fileと判定の正本は製品検証記録である。

未照合組合せは、全wire/link/STP/VID/aging/GCL同時刻、PSFP通過後queued copyのpolicy失効・token非refund・credit resetを重ねるfixture、全T±1ps境界とprefix比較、全機能無効のdynamic同値、全codec/予約/ID/metric failure地点である。実施したplanner/CLI/Viewer試験をIEEE 802.1Qbv/Qav/Qci conformance、clock同期品質、大規模長時間性能の証拠へ拡張しない。
