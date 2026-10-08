# Ethernet動的制御検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 2f1e60b`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）
文書ID：`verification-ethernet-dynamic`
文書状態：未公開。抽象モデルを実装済みで、native CLI・schema2出力・Viewerに対応する。実施した製品試験と未照合の組合せは検証仕様の実施記録に区別して示す。規格全体適合の証明ではない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | 初回pushに向け、動的制御・経路・時刻・上限・停止・互換性の独立期待値と製品照合範囲を規定 |

[仕様](../../specs/models/Ethernet動的制御詳細機能仕様書.md)・[設計](../../design/Ethernet動的制御詳細設計書.md)を対象とする。以下は実装の受入契約と独立期待値である。製品fixtureとnamed testの実施範囲を第5節に示し、ソース/binary hash・コマンド・成果物・判定を製品検証記録へ保存する。表中の全変種が照合済みという意味ではない。

設計段階の[独立算術vector](../fixtures/network-extensions/design-vectors.json)と[文書・設計検証記録](../results/network-extension-design-2026-10-07.json)を参照する。これらはcodec・時刻・credit・token・受信母数の期待値と文書整合性の証拠であり、製品用入力fixtureの実行結果ではない。

<a id="dir-test-0109"></a>

## 1. DIR-TEST-0109：入力・学習・期限

```trace
{"id":"DIR-TEST-0109","stage":"verification","requirements":["DIR-REQ-0240","DIR-REQ-0241","DIR-REQ-0242","DIR-REQ-0243"],"upstream":["design-ethernet-dynamic#membership"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0061。準備正常系はschema4、空generators/controls、cycle・parallel link・非連結、VID境界1/4094、IPv4/IPv6を各々含む。旧VLANと同じwire/tag/class算術を使うことを独立vectorで照合する。

| 負例 | 期待値 |
| --- | --- |
| schema不一致、未知/欠落/重複キー、control id重複、boolを整数/Dへ投入、不正D、負値、u64超過 | callback0でprepare拒否、入力位置/field/理由診断 |
| link未列挙/二重列挙/存在しないpair、bridge不足/重複ID、cost0、zero lifetime/mac_age | prepare拒否 |
| 無許可VID登録、静的VID登録/解除、動的untagged、VID0/4095 | prepare拒否 |
| IP family不一致、sourceがgroup、groupがunicast、MAC写像/EtherType不一致、IPv6表記別同一source重複 | prepare拒否。MAC collisionの異なるgroup自体は許可 |
| T以後又はgenerators空で上記不正を投入 | 実行されないことを理由に検証を省略しない |
| at+lifetime、at+convergence、tree cost加算overflow、controls上限+1 | prepare拒否、u128中間計算からchecked縮小 |

学習単体はSwitch SへFCS正常frameがt=100でport Aから到達、mac_age=1000とする。FDB entryは[100,1100)、t=500の同port受理でexpire=1500、t=700の別port Bから同source到達でmove=1・expire=1700となる。旧1100/1500期限は無効。t=1700のphase0で削除し、同時刻phase1で同source到達なら新entry期限2700。無効FCS/admit/VID/STP ingressでは学習0。static同key指定時はdynamic生成0・static_shadowed、staticの不適格portへknown lookupした場合flood0を確認する。

MAC表上限1で別MACを受理するとmac_capacity=1、既存entry保持、frame転送継続。既存MACのrefresh/moveは成功。削除再挿入時の世代再利用を禁止し、古い期限が新entryを消さないことを故障注入で確認する。topology変化によるflushと単なるmembership変更による非flushを分ける。

<a id="dir-test-0110"></a>

## 2. DIR-TEST-0110：源別group・VID登録

```trace
{"id":"DIR-TEST-0110","stage":"verification","requirements":["DIR-REQ-0240","DIR-REQ-0241","DIR-REQ-0242","DIR-REQ-0243"],"upstream":["design-ethernet-dynamic#membership"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0061。Sのingress=A、egress=B/C/D、全VID10静的member・forwardingとする。group239.1.1.1、source x=192.0.2.1/y=192.0.2.2、B=INCLUDE{x}、C=EXCLUDE{x}、D=router。xの候補はB/D、yはC/D。B=INCLUDE空ならx/yともBへ0、C=EXCLUDE空ならx/yともCへ1。static group BがあればBは動的filterと無関係にunionされ、一つのcopyのみ。IPv6でも同じ論理で試験する。

| ケース | 期待値 |
| --- | --- |
| membership既知だが全source不一致・routerなし・静的なし | 候補0、unknown_multicast=floodでもfloodしない |
| 最後のmembershipをleave/expire、router/staticなし | unknown policyに戻る。dropならunknown_multicast、floodなら現在eligible全egress |
| 異なるIP groupが同MACへ写像 | membershipは別key。source/groupが違うframeを誤配しない |
| ip_multicast=nullのL2 group | 動的源filter/routerを適用しない、旧静的lookup |
| membership set t=100 lifetime1000、refresh t=1100 lifetime1000 | controlを期限より先に処理しexpiry2100。1100の旧期限で削除0 |
| 同key同時刻id a=set、b=leave | id順で最後leave、候補は未登録扱い。idを逆にすれば登録が残る |
| member portのlink down/VID失効 | 状態は保持、候補eligibleから除外。復旧時に期限が過ぎていれば復活しない |
| Endpoint未購読 | Switch copyは存在、Endpoint receptionはmulticast_not_subscribed |

VID20はBのみregistrable（tagged）、t=100 register lifetime1000、t=600 refresh lifetime1000、t=1000 unregister、t=1000の後順id register lifetime500とする。有効区間は[100,1500)、旧1100/1600期限で新状態を変更せず1500に失効。A又はpeerに自動登録されない。VID10静的memberは全期間不変。VID20のqueued copyが1500に選択されるとvlan_unregistered drop、1499にSOF済みcopyは完了する。

membership/registration上限1で新key挿入を試みるとcallback失敗・state/epoch/record prefix未変更、同key更新は成功。sources_per_entry上限、timer bucket上限、世代overflowも検査する。期限大量refreshでpending予約数が更新回数に比例して増えないことを計測する。

<a id="dir-test-0111"></a>

## 3. DIR-TEST-0111：tree・切替・copy数値

```trace
{"id":"DIR-TEST-0111","stage":"verification","requirements":["DIR-REQ-0244","DIR-REQ-0245","DIR-REQ-0246","DIR-REQ-0247"],"upstream":["design-ethernet-dynamic#topology"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0062。S1/S2/S3のbridge_id=1/2/3、三角形各link cost10。root=S1、S2/S3 distance10、両者のroot portはS1向き。S2–S3 linkのdesignatedはS2、S3側はalternate/discardingで、このlinkはdataを通さない。S1–S3をdownにすると収束後S3 distance20・root port S2、S2–S3両端forwardingになる。parallel同costではneighbor/local port辞書順で一意に決まる。配列順を入替えて同結果を確認する。全link downでは三つのroot・孤立成分となりcross-component reception0。

時刻数値用fixture：A–S1–S2–Bの全hopはtagged VID10、1Gbps、payload0（MAC68byte）、link propagation1000ps、Switch processing2000ps、Endpoint TX/RX0。静的宛先経路、競合なし、初期forwardingとする。`wire=8*(68+8)=608 bit`、IFG込み704bitである。

| hop | SOF ps | EOF ps | release ps | arrival ps |
| --- | ---: | ---: | ---: | ---: |
| A→S1 | 0 | 608000 | 704000 | 609000 |
| S1→S2 | 611000 | 1219000 | 1315000 | 1220000 |
| S2→B | 1222000 | 1830000 | 1926000 | 1831000 |

B完了1831000ps = serialization1824000 + propagation3000 + processing4000 + queue_wait0。IFGはarrivalへ足さず送信器占有のみ。各copyのparent、offer/start/arrival epochも照合する。

同fixtureを独立に複製して次の制御を投入する。

| 条件 | 期待値 |
| --- | --- |
| S1–S2 down at611000 | phase0更新が先なので同時刻SOF0。queue copyはlink_down drop、S2 reception0 |
| S1–S2 down at800000、復旧なし | SOF611000のcopyはEOF1219000まで非中断。arrival1220000はlink_down filter、S2→B copy0 |
| down800000/up900000、convergence100000 | treeは1000000に再公開済み。on-wire copyはarrival1220000に受付、元経路でB1831000に完了 |
| convergence1000000、down800000/up900000 | 古い1800000収束を破棄し1900000で最新tree公開。arrival1220000はstp_discarding filter |
| queued B copy生成後にFDB/groupだけ変更 | 宛先Bのまま。現在link/VID/STPのみ再検査しreroute/追加fanout0 |
| 収束期限とlink_set同時刻 | link_setを先に適用、旧世代treeを一瞬も公開しない |

任意のon-wire copyがdown中にEOFとなってもwire/occupied bit集計を打切らない。phase2でgateが閉じていても不適格copyはdropし、TSN gate eligibilityとの共用契約を検査する。phase0 wire完了・link・membership・期限・GCLを同時刻に重ね、グローバルFESを変更せずcoordinator batchの所定順で唯一のstateを得る。

移動中のcopyがtree切替後に以前のSwitch/outputを再訪するfixtureを作る。visit IDとparentが唯一であること、frame@portの衝突を起こさないこと、parent graphが非巡回なことを確認する。visits_per_frame=2なら3回目の受理候補をvisit_limit filter、forwarding新copy0とし、無限送信を止める。

<a id="dir-test-0112"></a>

## 4. DIR-TEST-0112：停止・観測・互換性

```trace
{"id":"DIR-TEST-0112","stage":"verification","requirements":["DIR-REQ-0244","DIR-REQ-0245","DIR-REQ-0246","DIR-REQ-0247"],"upstream":["design-ethernet-dynamic#topology"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0062。前節全数値時刻、control時刻、aging/membership/VID expiryと収束時刻について、T=tとT=t+1で停止する。T=tはその時刻のeventを含まず、T=t+1は含む。終了条件の既存共通契約に従い、予定EOF/arrival/収束を実績にしない。途中停止ではpending/processing/on-wire/queuedの残量を保持し、未到達receptionを補わない。

失敗注入はcodec、control Delta構築、Effects予約、commit前record容量、Arrival学習+forwarding、tree公開、queue不適格dropに行う。callback直前state/epoch/records/queue bytesが失敗後も一致し、先行成功callbackは保持されることを確認する。汎用rollbackがあるという前提の試験にはしない。

独自5種record schemaと外枠schema2・5ファイルのJSON/CSV projection、manifest hash、初期policyと全controls metadata、epoch/visit/parent参照を照合する。学習/移動/期限/flushと明示no-opの母数（世代不一致wakeは実効監査へ数えない）、queue frames/bytes、reason別drop/filter、受信遅延4成分と未完了の扱いを独立集計する。実効変更なしbatchではepoch増分0、複数実効差分batchでは増分1。

Viewerはdown/alternate/quiet port、MAC aging、INCLUDE/EXCLUDE source、静的と動的VID、予定と公開tree、同port再訪を表示する。t前後seek、>2^53ps、forward/backward到達区間一致、連続線強調、offline/mobileを検査する。不正parent/epoch/schema/IP familyをloaderで拒否する。表示filterでsummary母数を変えない。

旧CAN/Gateway/CAN FD、Ethernet L2/QoS/VLAN/媒体、transaction profilesの既存fixtureを回帰し、旧schema・record ID・時刻・拒否fieldを保持する。動的profileでのみcycleが許可され旧VLANでは拒否されることを確認する。Rust fmt/clippy/locked tests、JS pure-model/browser、CLI validate/run/viewの実行結果は製品検証記録へ保存する。文書リンク/trace検査の成功をこれら製品試験の代用にしない。


## 5. 実施記録と照合範囲

[製品検証記録](../results/network-extension-product-2026-10-07.json)、[独立fixture](../fixtures/network-extensions/dynamic/expected.json)、[module単体試験](../../../crates/dir-simulator/src/runtime/ethernet/dynamic.rs)、[CLI統合試験](../../../crates/dir-simulator/tests/network_extensions.rs) を区別して参照する。設計時の記録と `design-vectors.json` の `product_execution:false` は変更しない。

| TEST ID | named test・製品経路 | この実施で照合する範囲 |
| --- | --- | --- |
| DIR-TEST-0109 | `strict_input_validates_future_controls_and_ip` / `learning_refresh_move_capacity_and_expiry` / `static_shadow_and_known_ineligible_do_not_flood` / `timer_refresh_bounded_and_generation_overflow_atomic` / `staged_learning_lookup_observes_delta_before_commit`、統合 `empty_and_control_only_workloads_run_on_the_same_prepared_profile` | 将来不正control/IP、学習refresh/move/expiry/capacity、static優先、期限取消/世代overflow、同callback学習後lookup、空負荷。負例表の全fieldを終了時刻ごとに掛け合わせた試験ではない |
| DIR-TEST-0110 | `source_filter_static_union_router_and_ip_collision` / `known_source_mismatch_does_not_flood_and_router_unions` / `control_refresh_at_expiry_and_registration_eligibility` / `atomic_capacity_failure_retains_state_and_epoch`、統合 `registrable_source_and_fdb_use_effective_membership_and_drop_expired_queue_copy` | INCLUDE/EXCLUDE・router・static union、MAC collision別group、known不一致、control優先refresh、VID期限/eligibility、上限時未commit、登録source/FDBの実転送とsource期限切れqueued copyのdrop。IPv6を含む全組合せ・Endpoint購読拒否・実FES大量refreshの性能を網羅した主張ではない |
| DIR-TEST-0111 | `tree_triangle_deterministic_and_break_before_make` / `isolated_components` / `parallel_same_cost_chooses_lexical_ports` / `stale_convergence_replaced_and_noop_epoch`、統合 `dynamic_triangle_matches_independent_three_hop_timing` | triangle、非連結、parallel決定性、旧収束無効/no-op、三hopの全SOF/EOF/release/arrivalとB完了1831000ps。linkdownを各wire境界へ配置した全行のCLI照合、実Switch再訪経路のfixtureは別途必要 |
| DIR-TEST-0112 | 統合 `time_limited_dynamic_run_keeps_planned_and_observed_receptions_separate`、共通 `staged_sof_overflow_preserves_admitted_copy_without_planned_transmission` / `staged_effect_validation_keeps_frame_queue_and_failed_event_uncommitted` / `staged_publication_reservation_failure_keeps_fes_and_journal_prefix`、Viewer model/browser | T=700000psで予定EOFと実績を分離、失敗時queue/FES/journal prefix保持、policy/epoch/visit参照検証、前後seek・quiet topology。全T=t/t+1ps、全失敗地点、全旧profile入力変種の成功数ではない |

完全入力例は [unicast](../../../examples/ethernet/dynamic/unicast.ini)、[empty](../../../examples/ethernet/dynamic/empty.ini)、[topology](../../../examples/ethernet/dynamic/topology.ini)、[membership](../../../examples/ethernet/dynamic/membership.ini)、[registration](../../../examples/ethernet/dynamic/registration.ini)。主な実施commandは `/home/hideki/.cargo/bin/cargo test --locked -p dir-simulator --lib ethernet::dynamic`、`cargo test --locked -p dir-simulator --test network_extensions`、`node --test tests/network_viewer_model.test.cjs`、`node tests/network_viewer_browser.cjs <製品results.json> ...`。正確なcommand、対象成果物、全回帰結果は製品検証記録を正本にする。

未照合の受入組合せとして、全wire/control/expiry/収束時刻のT±1ps、wire進行中のlinkdown/up全配置、以前のSwitch/outputへ戻る実再訪とvisit_limit、phase0の全domain同時刻合成、全preflight失敗地点を残す。仕様のtable/timer上限保証と試験した境界は区別し、大規模性能やBPDU/IGMP/MLD/MVRP protocol conformanceの証拠にしない。

統合試験 `registrable_source_and_fdb_use_effective_membership_and_drop_expired_queue_copy` は宣言済みregistrable VIDのsource/FDB参照、時刻0登録、未登録source drop、登録失効後のqueued copy再検査、PVID静的member必須を照合する。`dynamic_large_cost_tree_and_at_prefixed_control_id_remain_valid` はu64内の大きなcostで初期/更新treeを生成し、入力ID `@maintenance` の監査値を保存する。native generatorの同時刻重複は `[a:0,a:1,b:0]` の順を維持する。製品記録へ実施結果を保存する。
