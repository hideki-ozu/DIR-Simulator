# CAN・Ethernet変換検証仕様書

文書バージョン：`1.1.0`
設計日：`2026-10-07`
対象GitHubバージョン：`main @ 2f1e60b`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）
文書ID：`verification-can-ethernet`
文書状態：未公開。抽象モデルを実装済みで、native CLI・schema2出力・Viewerに対応する。実施した製品試験と未照合の組合せは検証仕様の実施記録に区別して示す。規格全体適合の証明ではない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | 初回pushに向け、CAN↔Ethernet変換の独立codec・時間・容量・lineage・受信母数期待値と製品照合範囲を規定 |

[仕様](../../specs/models/CAN・Ethernet変換詳細機能仕様書.md)、[設計](../../design/CAN・Ethernet変換詳細設計書.md)、[DIR-AC-0059/0060](../../要件定義書.md)に対応する。各ケースは詳細契約と独立期待値を示す。実装済みproductのnamed testと実行例に対応する照合範囲を第5節に示し、下表の全行・全変種を試験済みとは扱わない。独立照合器は整数算術、固定byte vector、保存則を正本にし、製品serializerを呼んで期待値を作らない。

設計段階の[独立算術vector](../fixtures/network-extensions/design-vectors.json)と[文書・設計検証記録](../results/network-extension-design-2026-10-07.json)を参照する。これらはcodec・時刻・credit・token・受信母数の期待値と文書整合性の証拠であり、製品用入力fixtureの実行結果ではない。

<a id="dir-test-0105"></a>

## 1. codecと正入力

```trace
{"id":"DIR-TEST-0105","stage":"verification","requirements":["DIR-REQ-0230","DIR-REQ-0231","DIR-REQ-0232","DIR-REQ-0233"],"upstream":["design-can-ethernet#codec"],"state":"confirmed","pending":[]}
```

| ケース | 入力 | 独立期待値 |
| --- | --- | --- |
| C01 | CAN ID0/DLC0 | codec=`4449524301000000000000`、11byte、padding35byte、untagged MAC64byte、tagged68byte |
| C02 | ID291/DLC2/aabb | codec=`4449524301000000012302aabb`、13byte、padding33byte、CAN decodeでID291/data aabb |
| C03 | ID2047/DLC8、全FF | codec=`444952430100000007ff08ffffffffffffffff`、19byte、padding27byte |
| C03E | extended ID536870911/DLC8、全FF | codec=`4449524301011fffffff08ffffffffffffffff`、19byte、padding27byte、flags=1、最大29bit IDを復号 |
| C03F | standard/extendedのID0とID2047を各々入力 | 同じ数値でも形式を保存。extended ID2048も正常、standard ID2048は拒否 |
| C03G | extended ID291→Ethernet→standard ID1の出力rule | codec flags1/ID291で照合、出力standard ID1、元/出力formatを別記録、空dataの出力CAN47bit |
| C04 | VID10/PCP3/DEI0 | TCI=`600a`、tag=`8100600a`、inner EtherType=`88b5`。data/padding保存と独立CRC32でFCS確認 |
| C05 | ID0→PCP3、PCP3→ID1 | Ethernet source class3、復号後CAN ID1、空payloadはCAN47bit（ID0の50bitを流用しない） |
| C06 | tagged→untagged、default_priority=1 | 中継前class3、受信後class1。to_can PCP3規則はno_rule |
| C07 | 二CAN Bus、一Ethernet tree、二Gateway | 一つのFES、CAN/Ethernet独立資源の同時SOF可、所有集合・対応NED pathが一意 |

<a id="dir-test-0106"></a>

## 2. 準備・受信拒否

```trace
{"id":"DIR-TEST-0106","stage":"verification","requirements":["DIR-REQ-0230","DIR-REQ-0231","DIR-REQ-0232","DIR-REQ-0233"],"upstream":["design-can-ethernet#codec"],"state":"confirmed","pending":[]}
```

| ケース | 変更 | 期待 |
| --- | --- | --- |
| N01 | 未知version/magic、予約flags bit1=1、DLC9、standard ID2048、extended ID536870912、10byte不足、nonzero末尾 | 到着時invalid_codec、CAN request生成0、診断対象byte offset固定 |
| N01F | flags0でID2048、flags1でID536870912、format欠落rule、match形式不一致 | 前二者invalid_codec、欠落はprepare拒否、合法codecと異なるmatch形式はno_rule |
| N02 | bad FCS、非member VID、他individual MAC | 通常媒体拒否、conversion attemptedへ加算0 |
| N03 | RX0で正codec到着、rule非該当到着 | 前者rx_full、後者no_rule。判定順どおり各1理由のみ |
| N04 | CAN FD/RTR、TSN、動的table、半二重 | prepare失敗。count=0/将来時刻でも拒否 |
| N05 | Gateway重複所有、未接続egress、Ethernet cycle、同一egress重複 | prepare失敗、JSON pointer/NED pathあり |
| N06 | bool PCP、PCP8、VID0/4095、時刻u64超過、重複rule key、未知key | prepare失敗、run未開始 |
| N07 | EtherType別値又は未購読multicast | 媒体受信通過なら別EtherTypeはinvalid_codec、未購読は媒体拒否 |

<a id="dir-test-0107"></a>

## 3. 双方向時刻・容量・母数

```trace
{"id":"DIR-TEST-0107","stage":"verification","requirements":["DIR-REQ-0230","DIR-REQ-0234","DIR-REQ-0235","DIR-REQ-0236","DIR-REQ-0237","DIR-REQ-0238","DIR-REQ-0239"],"upstream":["design-can-ethernet#runtime"],"state":"confirmed","pending":[]}
```

単位usを使う表でも出力はps整数完全一致とする。CAN ID0空payloadはCRC/stuffing込み50bit、500kbpsでEOFまで100us、intermission3bitでrelease106us。1Gbps Ethernetのuntagged64byteはpreamble/SFD8byte込み576bit=0.576us、IFG込み672bit=0.672us。tagged68byteは608bit=0.608us、IFG込み704bit=0.704us。

| ケース | 入力 | 独立期待値 |
| --- | --- | --- |
| R01 CAN→Eth | CAN SOF0、EOF100us、CAN伝搬0、rx処理1us、変換2us、eth tx処理3us、untagged、link伝搬4us、sink rx5us | Gateway observed100、ready103、offer/SOF106、eth EOF106.576、release106.672、sink observed110.576、received115.576us。RXは100..106us、e2e115576000ps |
| R02 Eth→CAN | eth SOF0、tagged、伝搬1us、GW rx2us、変換3us、can tx4us、500kbps、ID0/DLC0、sink rx5us | eth EOF0.608、GW observed1.608、ready6.608、CAN SOF10.608、EOF110.608、release116.608、sink received115.608us |
| R03 fanout | R02の出口CAN A=500kbps、B=250kbps、両方空 | SOF両方10.608、EOF A110.608/B210.608、release A116.608/B222.608、RX release10.608us |
| R04 TX待ち | 同時readyの二枝、A空、B満杯で先行送信SOF20usがslotを解放 | Aはreadyに受理、Bは20us再試行、RXは20usまで保持。A child一個のみ、B一個、CAN Bは媒体release後に仲裁 |
| R05 永久不能 | B容量0又はEth class byte容量63で64byte frame | B tx_unadmittable、A正常受理。全枝terminal後RX解放、waiting永久残留0 |
| R06 RX2/同時3到着 | 同一時刻、origin a,b,c順、正処理遅延 | a/b受理、c rx_full、max2、attempted3=accepted2+rejected1 |
| R07 資源同時刻 | EOF/release/offerが同t、CAN ID1/ID2候補 | completion→offer→arbiter、ID1先。phase2解放からの再offerは次delta、seq/入力配列順で結果不変 |
| R08 multicast | 同VIDのGW1/GW2/sinkが購読、通常endpoint xは未購読 | Ethernet母数3、xは含めない、各GW conversion1、sink受信1。Switch hopは母数へ加算しない |
| R09 CAN母数 | 送信元+Gateway+通常sink、両受信filter一致 | CAN母数2、Gateway処理未完了でもCAN媒体受信成功と変換releasedを混同しない |
| R10 CAN終端2台 | 一つの変換child branchが同一busの非Gateway sinkA/sinkBへ送信、両filter一致 | segment対象tuple2、SOFでtarget_count2、両受信処理完了時completed2、end-to-end率2/2=1。一枝を分母として2/1にしない |
| R11 Ethernet終端2台 | 一つのEthernet broadcast変換branch、同VIDの非Gateway endpoint A/B、途中SwitchとGateway受信者あり | segment対象tuple A/Bの2、Switch hopで分母増加0、中間Gatewayで増加0、両完了で2/2=1 |
| R12 native元送信 | Gatewayを通らないnative CAN又はEthernet送信から2 terminal | native segment IDと空branch_lineageで対象2、両完了2/2、conversion record無しでも再構築可 |
| R13 lineage別到着 | 同originの別segment/lineageから同terminalへ2受信 | 異なるtuple2、target_count2、両完了2。片方の同tuple重複通知を足してもcompleted2 |

R01のEthernet出力をtaggedへ変えるとEOF/arrival/receivedは各32000ps増、SOFは不変。任意の一遅延を1ps増やす独立変化試験で、その遅延以後の因果経路のみ1ps増えることを照合する。busyはCAN100us+intermission6us、Ethernet0.576us+IFG0.096usとして媒体別に集計する。

<a id="dir-test-0108"></a>

## 4. lineage・partial・失敗・viewer回帰

```trace
{"id":"DIR-TEST-0108","stage":"verification","requirements":["DIR-REQ-0230","DIR-REQ-0234","DIR-REQ-0235","DIR-REQ-0236","DIR-REQ-0237","DIR-REQ-0238","DIR-REQ-0239"],"upstream":["design-can-ethernet#runtime"],"state":"confirmed","pending":[]}
```

| ケース | 入力/停止点 | 期待 |
| --- | --- | --- |
| P01 | GW1→Eth→GW2→CAN→GW1の循環 | 最後のGW1到着はloop_prevented。訪問列GW1/GW2、無限転送なし。別枝の同origin到着はglobal重複で落とさない |
| P02 | max_hops=1、2個目Gateway | 2個目はhop_limit。Wireだけ再投入した別native frameは新origin |
| P03 | R01のT=100us | CAN EOF未発火、conversion0、予定EOFからreceiver生成0 |
| P04 | R01のT=103us | RX processing、ready null/planned103us。T=103us+1psではready103、TX processingでRX保持 |
| P05 | R01のT=106us、106us+1ps | 前者branch未admitted・SOF null、後者SOF106us/EOF null、RX解放、sink成功0 |
| P06 | R01のT=115.576us | sink observed済み、received null、end-to-end completed0。T+1psで1 |
| P07 | R04途中停止/再生 | 一枝admitted、一枝waiting、RX1。前後移動で同時刻状態と移動区間が一致 |
| P08 | 全枝preflightの最後でu64時刻/ID/byte/metric overflowを注入 | callback全体未commit、先行枝の新record/queue変更0、直前成功prefix維持、失敗event pending |
| P09 | try_reserve失敗、effect validator失敗、event limit各境界 | P08と同じ。以前成功したarrival processing行を削除せずreadyを捏造しない |
| P10 | 入力Gateway/rule/egress配列逆順 | canonicalized manifest以外の出典差を除きrecord ID/順序/時刻/metric完全一致 |
| P11 | 壊れたparent/child、未知schema/version、PCP不一致 | viewer loaderが明確に拒否。正常partialのnull childは拒否しない |
| P12 | 通常再生/step forward/back/quiet表示 | 連続は線強調、stepは実績媒体移動、Gateway待機表示、未来受信生成なし |
| P12E | R10/R11で一方の受信処理だけ完了した時点T | target_count2/completed1で率1/2。未完了tupleのcompleted_psはnull、予定完了を分子へ加算しない |
| P12F | SOF前停止、対象が中間Gatewayだけ、重複completion通知 | 前二者end-to-end分母0/率null。後者同tupleのcompleted/遅延増分0。segment recordだけから集計を独立再構築 |
| P13 | 既存13 profiles全fixture | profile/codec/時刻/schema/ID不変。新profile opt-inなしに新record混入0 |

全checkpointで `attempted=accepted+rejected`、`accepted=processing+waiting_tx+released`、RX occupancy=processing+waiting_tx、枝保存則および`0≤end_to_end.completed≤end_to_end.target_count`を照合する。waiting_txにはTX処理中で未受理の枝を持つconversionを含む。null平均、母数0、T=0、最大整数、複数ingress、正/ゼロ遅延を含む。

## 5. 実施記録と照合範囲

[製品検証記録](../results/network-extension-product-2026-10-07.json) は実行command、toolchain、source/fixture hash、成果物と結果を保存する。[独立製品fixture](../fixtures/network-extensions/can-ethernet/expected.json) の期待値は製品serializerを呼ばず作成した。設計時の記録と `design-vectors.json` の `product_execution:false` はそのまま残し、今回の製品実行を過去の記録へ混入させない。

| TEST ID | named test・製品経路 | この実施で照合する範囲 |
| --- | --- | --- |
| DIR-TEST-0105 | codec `fixed_vectors`、input `complete_native_r01_r02_inputs_validate`、統合 `can_ethernet_product_preserves_r01_and_r02_processing_and_wire_times` | C01/C02/C03/C03Eの固定byte、standard/extended境界、完全native入力、R01/R02のserializerと媒体因果。C04の全CRC変種、C06の中継再分類、C07の二Gateway組合せを全部実行した主張ではない |
| DIR-TEST-0106 | codec `every_invalid_field_and_padding`、input `strict_rules_ownership_and_ranges_reject` / `source_ownership_checked_for_zero_count_and_future_time`、runtime `rx_capacity_and_loop_hop_rule_order` | codec不正field/offset、rule/所有/範囲、count0/将来所有source、loop/hop/no_rule/RX判定順。媒体FCS・購読・全不正組合せをN01〜N07ごとに網羅した集計ではない |
| DIR-TEST-0107 | runtime `r01_exact_store_and_forward_and_rx_hold` / `r02_exact_timing_native_can_and_terminal_completion` / `fanout_buses_start_together_with_independent_wire_times` / `fanout_wait_keeps_rx_and_retries_only_unadmitted_branch_at_sof` / `zero_delay_admits_and_unadmittable_terminates` / `staged_multiple_ingress_capacity_and_error_atomicity` / `can_controller_queue_uses_can_arbitration_priority`、統合 `bridge_fanout_rx_limit_and_partial_denominators_preserve_native_contracts` | R01/R02の実時刻、R03二Bus、R04 RX保持とSOF再試行、R05永久不能、R06同時容量、CAN優先仲裁、segment実SOF対象と重複completion。RX-limit実行例はRX1・3到着を追加照合 |
| DIR-TEST-0108 | runtime `segment_denominator_freezes_and_duplicate_completion_does_not_increment` / `future_native_generation_flag_without_synthesized_rows` / `malformed_timer_handles_and_impossible_transition_do_not_mutate`、共通 `staged_effect_validation_keeps_frame_queue_and_failed_event_uncommitted` / `staged_publication_reservation_failure_keeps_fes_and_journal_prefix`、統合partial、Viewer model/browser | 実SOF分母・null未来実績・重複通知、未来生成、失敗callback保持、T=101000000psでnative CAN pendingかつEthernet受信なし、正常/不正prefix replay。全P03〜P06のT±1ps組合せや全preflight地点への故障注入の合格を意味しない |

完全入力例は [R01](../../../examples/can-ethernet/r01.ini)、[R02](../../../examples/can-ethernet/r02.ini)、[双方向](../../../examples/can-ethernet/bidirectional.ini)、[fanout](../../../examples/can-ethernet/fanout.ini)、[RX-limit](../../../examples/can-ethernet/rx-limit.ini)。主な実施commandは `/home/hideki/.cargo/bin/cargo test --locked -p dir-simulator --lib can_ethernet`、`cargo test --locked -p dir-simulator --test network_extensions`、`node --test tests/network_viewer_model.test.cjs`、`node tests/network_viewer_browser.cjs <製品results.json> ...`。共通format/clippy/全profile回帰とbrowser対象fileの正確な結果は製品検証記録を正本にする。

残る照合範囲は、P01の複数Gatewayをwireで一周する完全fixture、R08/R11のSwitch multicastを含む終端集合、R01のtag切替・各遅延1ps変化、全T境界の直前/直後、全fault-injection地点、P10の全入力順反転である。これらの詳細期待値は受入契約として残す。実施した代表経路の成功を、表の全subcase合格、長時間/大規模性能、外部規格適合へ拡張しない。

独立レビューの再現ケースも製品記録へ保存する。同名CAN/Ethernet generatorは媒体修飾originで区別し、生成時刻の大小を反転した統合試験 `identical_native_generator_ids_keep_media_qualified_origins_and_latencies` を実行する。共有Port容量解放で別PCP枝を再試行する `shared_ethernet_port_retry_wakes_all_classes_in_stable_order`、受信admit拒否・静的FDB誤経路でも意図したSOF対象を保持するruntime試験を追加する。準備診断は `network_prepare_diagnostics_use_original_json_source_and_pointer` で変換前のJSON位置へ結び付ける。正確なnamed testの一覧と結果は製品記録に保存する。

`conversion_upserts_preserve_milestone_times_on_retries_and_late_sof` と `same_batch_branch_sof_emits_each_changed_branch_once` は、容量不変の再試行、子SOFと兄弟branch、最終branchのRX解放、同batchの重複通知を照合する。DTOが変わらない行を再発行せず、conversion自身の実績時刻を保持する。
