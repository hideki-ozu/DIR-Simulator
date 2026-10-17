# wire遷移公開catalog

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.6`
文書ID：`proposal-implementation-wire-transition-catalog`
文書状態：承認済み限定scopeの公開射影契約。実行検証の最終判定は別記録。

### 更新履歴

| 版 | 日付 | 内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-17 | schema 1のclosed型、機械別有限catalog、メッセージ不在表現、PRS観測scope、精度境界を明文化。v1.1.6収録版を確定 |

## 1. 適用範囲

[統合設計§5.7](RSTP・MVRP-wire実装設計書.md)、[RSTP補足](RSTP規範仕様・詳細設計書.md)、[MVRP補足](MVRP規範仕様・詳細設計書.md)の公開transitionを補足する。[公開ガイド](../../guide/index.md)のv1.1.6収録の実装に対応する。規範変数とDIR内部実装の観測値を区別する。新しい機械・protocol・公開event・schema versionを追加しない。IEEE全体の適合性を主張しない。

有限catalogと型の実装は `crates/dir-simulator/src/runtime/wire/transition.rs`。producerと `output/wire.rs` のvalidatorは同じcatalogを使う。private Port/Bridge/Attribute、checkpointのserde表現は維持し、全文before/afterから公開値を作る。未知source field、不明な変更、別機械変数の混入を失敗にする。文字列化による任意値の受入れは行わない。

## 2. schema 1の外側

`WireTransitionRecord` のdomain objectは次の**13キーを全て必須**とし、追加キーを拒否する。公開model recordのcarrierではhostが共通の `effect_seq` を追加するため**14キー**になる。`output/network/wire.rs` が必須の正準u64文字列 `effect_seq` を検証してから外し、残るdomain objectを本catalogで検証する。schema versionの追加ではない。

`schema_version, adapter_id, bridge, port, protocol, time_ps, callback_ordinal, transition_ordinal, machine, from_state, to_state, trigger, changes`

`schema_version` はJSON整数1。識別子は空でない文字列。`protocol` は `rstp` 又は `mvrp`。time/ordinalは後述の正準u64文字列。`port` キーの省略は拒否する。`port:null` はbridge PRSだけ。その他は実際のport ID文字列。PRSのport付き観測行は§5の限定例外。

`changes` は有限catalogの変数名をキーにしたobject。各値は**ちょうど** `{before: typed_value, after: typed_value}`。両側必須、追加キー・null・型違いを拒否する。空changesの実在transition、UCT、自ループを保持する。stateは外側from/toに置き、raw `pim/prx/...` を変数にしない。`UnknownValue`、任意object、機械間の全変数unionは許可しない。

## 3. 値の型

| 型 | 厳密な表現 |
| --- | --- |
| u64 | ASCIIの正準10進文字列 `0` 又は先頭0なしの数字列、0..18446744073709551615。JSON number、符号、空白、小数、指数、overflowを拒否 |
| Priority | ちょうど7個のu64文字列の配列。順序は RootID, ExternalCost, RegionalRootID, InternalCost, DesignatedBridgeID, DesignatedPortID, ReceivingPortID |
| Times | ちょうど `MessageAge_ticks, MaxAge_ticks, HelloTime_ticks, ForwardDelay_ticks` の4キー。それぞれ正準10進文字列0..65535。単位は1/256秒 |
| bool | JSON true/falseのみ |
| Role | Disabled, Root, Designated, Alternate, Backup |
| Info | Disabled, Aged, Mine, Received |
| ReceivedInfo | None, SuperiorDesignatedInfo, RepeatedDesignatedInfo, InferiorDesignatedInfo, NotDesignatedInfo, OtherInfo |
| MessageKind | None, Config, Tcn, Rst |
| MessageVersion / Flags | 正準10進文字列0..255 |
| attribute | 正準10進VID文字列1..4094、beforeとafterは同じ識別子 |
| PortSet | 空配列又は昇順・重複なしの空でないport ID文字列配列。JSON number/objectを拒否 |

`rootPortId` と10種類のcounterはu64型。current reducerのu16/u32というprivate保存型とは分離する。priority内のbridge IDは `9223374235878031361`、正確な整数一般の例は `9007199254740993`、u64最大値も丸めず両側の文字列を保持する。codecの実field幅制約は別途維持する。

### 3.1 RSTP sourceからの写像

| private source | 公開変数・型 |
| --- | --- |
| enabled / selected_role / info / update | portEnabled bool / selectedRole Role / infoIs Info / updtInfo bool |
| priority / designated / root | portPriority / designatedPriority / rootPriority、Priority |
| times / designated_times / root_times | portTimes / designatedTimes / rootTimes、Times |
| root_port | rootPortId、u64 |
| counters.edge / hello / migration / fd / rr / rb / info / tc / detected / tx | edgeDelayWhile / helloWhen / mdelayWhile / fdWhile / rrWhile / rbWhile / rcvdInfoWhile / tcWhile / tcDetected / txCount、u64 |
| send_rstp / rcvd_stp / rcvd_rstp / oper_edge / reroot / new_info / tc_ack / rcvd_tc / rcvd_tcn / rcvd_tc_ack / tc_prop / fdb_flush | sendRSTP / rcvdSTP / rcvdRSTP / operEdge / reRoot / newInfo / tcAck / rcvdTc / rcvdTcn / rcvdTcAck / tcProp / fdbFlush、bool |
| role | role、Role |
| selected / reselect / mcheck / isolate / proposing / proposed / agree / agreed / disputed / sync / synced / learn / forward / learning / forwarding | 同名bool |
| received_info | rcvdInfo、ReceivedInfo。Option不在はNone |

private `config` は既知の不変設定としてaccountし、変更を拒否する。port機械stateは既知の8キーとしてaccountし、自機械stateは外側に表す。他機械stateの未説明の変更を黙って落とさない。bridge PRSの入力は実在するroot/root_times/root_portを全て含む両側object、又は既知の完全Bridge。不変id/times/holdと別機械ptiは変更不可。Bridgeの子portへのPRS書込みは別の観測行に記録する。

### 3.2 メッセージbuffer

`message` は**現在処理待ち**のbufferであり、`last_message` は**最後に保持した受信内容**。後者を現在のpending packetと混同しない。

| buffer | 公開変数 | 書込み観測機械 |
| --- | --- | --- |
| message | rcvdMsg bool, msgPriority Priority, msgTimes Times, pendingMessageKind MessageKind, pendingMessageVersion / pendingMessageFlags | PRX、PIM |
| last_message | retainedMessagePresent bool, retainedMessagePriority Priority, retainedMessageTimes Times, retainedMessageKind MessageKind, retainedMessageVersion / retainedMessageFlags | PRX |

presenceとkind/version/flags、retained群はbufferを説明するDIR実装観測値。priority/timesは対応bufferの値。buffer不在時はpresence=false、kind=None、priorityは7個の`"0"`、Timesは全4値`"0"`、version/flagsは`"0"`。これらは**不在の型付きsentinel**であり、受信packetや履歴を捏造した値ではない。beforeも実際のbuffer状態から計算する。PIMがpendingを消費してもretainedを消したと報告しない。`rcvdBpdu` の別のprivate状態は存在しないため合成しない。

## 4. 機械別の有限集合

以下は機械ごとの許可集合。空changesを許可することは未知変数や未知stateを許可する意味ではない。

| protocol / machine / scope | 許可state | 許可変数 |
| --- | --- | --- |
| rstp / PTI / port | ONE_SECOND, TICK | edgeDelayWhile, helloWhen, mdelayWhile, fdWhile, rrWhile, rbWhile, rcvdInfoWhile, tcWhile, tcDetected, txCount |
| rstp / PRX / port | DISCARD, RECEIVE | portEnabled, rcvdSTP, rcvdRSTP, operEdge, isolate, edgeDelayWhile, rcvdMsg, msgPriority, msgTimes, pendingMessageKind, pendingMessageVersion, pendingMessageFlags, retainedMessagePresent, retainedMessagePriority, retainedMessageTimes, retainedMessageKind, retainedMessageVersion, retainedMessageFlags |
| rstp / PPM / port | CHECKING_RSTP, SELECTING_STP, SENSING | mcheck, sendRSTP, mdelayWhile, rcvdSTP, rcvdRSTP |
| rstp / BDM / port | EDGE, NOT_EDGE, ISOLATED | operEdge, isolate |
| rstp / PTX / port | TRANSMIT_INIT, TRANSMIT_PERIODIC, TRANSMIT_CONFIG, TRANSMIT_TCN, TRANSMIT_RSTP, IDLE | helloWhen, txCount, newInfo, tcAck |
| rstp / PIM / port | DISABLED, AGED, UPDATE, CURRENT, RECEIVE, SUPERIOR_DESIGNATED, REPEATED_DESIGNATED, INFERIOR_DESIGNATED, NOT_DESIGNATED, OTHER | infoIs, portPriority, portTimes, updtInfo, proposing, proposed, agree, agreed, synced, disputed, reselect, selected, newInfo, rcvdInfoWhile, rcvdTc, rcvdTcn, rcvdTcAck, rcvdInfo, rcvdMsg, msgPriority, msgTimes, pendingMessageKind, pendingMessageVersion, pendingMessageFlags |
| rstp / PRS / bridge | INIT_TREE, ROLE_SELECTION | rootPriority, rootTimes, rootPortId |
| rstp / PRS / port | INIT_TREE, ROLE_SELECTION | selectedRole, designatedPriority, designatedTimes, updtInfo, selected, reselect |
| rstp / PRT / port | INIT_PORT, DISABLE_PORT, DISABLED_PORT, ROOT_PORT, ROOT_DISCARD, ROOT_PROPOSED, ROOT_AGREED, ROOT_SYNCED, REROOT, REROOTED, ROOT_LEARN, ROOT_FORWARD, DESIGNATED_PORT, DESIGNATED_DISCARD, DESIGNATED_PROPOSE, DESIGNATED_AGREE, DESIGNATED_SYNCED, DESIGNATED_RETIRED, DESIGNATED_LEARN, DESIGNATED_FORWARD, BLOCK_PORT, ALTERNATE_PORT, ALTERNATE_PROPOSED, ALTERNATE_AGREED, BACKUP_PORT | role, fdWhile, rrWhile, rbWhile, edgeDelayWhile, sync, synced, reRoot, learn, forward, proposing, proposed, agree, agreed, disputed, newInfo, syncPorts, reRootPorts |
| rstp / PST / port | DISCARDING, LEARNING, FORWARDING | learning, forwarding |
| rstp / TCM / port | INACTIVE, LEARNING, DETECTED, ACTIVE, NOTIFIED_TCN, NOTIFIED_TC, PROPAGATING, ACKNOWLEDGED | tcWhile, tcDetected, tcAck, rcvdTc, rcvdTcn, rcvdTcAck, tcProp, fdbFlush, newInfo, tcPropPorts |
| mvrp / Applicant / port | VO, VP, VN, AN, AA, QA, LA, AO, QO, AP, QP, LO | attribute, demand, request_tx |
| mvrp / Registrar / port | IN, LV, MT | attribute, registered |
| mvrp / LeaveAll / port | Active, Passive | restart_timer, send_leaveall, request_tx |
| mvrp / Periodic / port | Active, Passive | restart_timer, periodic |

## 5. 複数portに対する実在entryの観測

PRSはbridge機械であり、通常のroot選択行は `port:null`。同じPRS entryが実際に書き込んだportについてのみ、`machine:PRS, port:実port` の観測行を追加する。from/to/triggerは同じ実在PRS entryのもの。許可変数は表のport PRS集合だけ。これは同じentryの書込みscopeを示す観測行であり、PRSをportごとに再実行したことや別の機械entryを意味しない。entryを再実行してbefore/afterを得ない。全行のtransition_ordinalを一意にする。Initializeも初期化された実rootをbeforeに使い、空object/nullからの擬似変化を作らない。

PRTのtree-wide sync/reRoot、TCMの他portへのtcPropは、それぞれ `syncPorts/reRootPorts/tcPropPorts` という**実装観測値**にする。source entry前後の実際の各flag=trueのport集合をPortSet型で記録する。対象portに架空のPRT/TCM entryを作らない。変更がなければ当該観測変数を省略する。外部LinkによるportEnabledは共通eventのport_policy.before/after.link_upでも観測でき、Link自体を架空のFSM entryにしない。

## 6. MVRP

Applicant/Registrarは全文Attributeの両側から**自機械の変数だけ**を選ぶ。stateは外側from/to。両機械の遷移行にはattributeを必須とし、VIDはbefore=afterの文字列で保持する。Applicantにregistered、Registrarにdemand/request_txを混ぜない。外側vidキーは追加しない。

LeaveAll/Periodicにはattributeを出さない。両機械stateはActive/Passive。表のaction flagは永続的IEEE変数ではなく、entry actionのDIR実装観測値。各entryでbefore=false、after=実際のaction結果boolに初期化する。false→falseの値は省略できる。他補助機械のaction flagがtrueの場合はproducerの不整合として失敗する。現在Traceはpersistent timerを書かず、timerのtoken/deadlineは既存event/effect journalが所有する。未知timer変数をcatalogへ暗黙追加しない。

## 7. guard

triggerは既知protocol/machineの `BEGIN` 又は `guards.rs` の**固定有限lookup**で認めた識別子だけ。producerは実際に選んだruleから専用fixture ID、又は未被覆行の `<machine>/rule/<frozen-section>/<row-index>` を得る。validatorは同じprotocol/machine別catalogと完全一致で比較する。prefix、raw event、未知機械、範囲外・非正準番号、別機械IDは受理しない。catalogは初回に一度だけ構築し、recordごとに再生成しない。

### 7.1 RSTPの実guardと凍結行

専用のone_guard_transitionはV218、V221–V262を実際のsource/destinationと対応させる。V219/V220は再entryなしstayなのでtaken transition用IDにしない。特に以下を分離する。

| 実分岐 | 公開ID |
| --- | --- |
| PRX DISCARD→RECEIVE | P-RSTP-T01-V218 |
| PPM SELECTING_STP→SENSING、enabledかつ!mcheckかつmdelayWhile=0 | P-RSTP-T01-V221 |
| 同じ出口のdisabled又はmcheck（expiryも同時成立ならこちら） | PPM/rule/13.32/6 |
| PPM SENSING→CHECKING_RSTP、enabledかつ!mcheckかつ!sendRSTP&&rcvdRSTP | P-RSTP-T01-V222 |
| 同じ出口のdisabled又はmcheck | PPM/rule/13.32/4 |
| BDM NOT_EDGE→EDGE、auto-edge条件で入り、disabled&&AdminEdgeではない | P-RSTP-T01-V223 |
| BDM NOT_EDGE→EDGE、disabled&&AdminEdge | BDM/rule/13.33/3 |

上記の多義的出口は実before Port/config必須。欠落・不成立条件から代表fixtureを選ばない。既存の関数signatureを維持し、FSMは変更しない。その他のfallbackは[RSTP規範設計](RSTP規範仕様・詳細設計書.md)の採用表の行順を凍結する。section番号は元機械の規範節、行はDIR設計表の行（IEEE本文の物理行番号ではない）。BEGIN行も数える。専用fixtureのないUCTは、そのtransient stateと戻りを記載した同じ表行を使う。

| section/machine | 受理するfallback行と意味 |
| --- | --- |
| 13.30 / PTI | 1=tick、2=UCT（設計§7冒頭の固定2行） |
| 13.31 / PRX | 1=DISCARD reset、3=RECEIVE再entry |
| 13.32 / PPM | 2=CHECKING self-reset、3=CHECKING→SENSING、4=非fixtureのSENSING reset、5=SENSING→SELECTING、6=非fixtureのSELECTING reset |
| 13.33 / BDM | 3=admin-edge、4=ISOLATEDへの入口 |
| 13.34 / PTX | 1=reset、2=TRANSMIT_INIT→IDLE、3=periodic、4=RSTP、5=TCN、6=Config（3–6は各送信stateからのIDLE戻りも同じ行） |
| 13.35 / PIM | 1=disabled、4=AGED→UPDATE、5=UPDATE→CURRENT、8–12=受信分類、13=分類state→CURRENT |
| 13.36 / PRS | 2=INIT_TREE→ROLE_SELECTION（再選択selfはV230） |
| 13.37 / PRT | 1=INIT_PORT戻り、2=Disabled選択、3=Disabled self、4=Root選択/更新、5–12=各Root transient戻り、13=Designated選択、14–20=各Designated transient戻り、21=Alternate/Backup選択、22=Alternate self、23–25=各Alternate/Backup transient戻り |
| 13.39 / TCM | 4=LEARNING flag-clear self、5=DETECTED戻り、7=NOTIFIED_TCN戻り、8=NOTIFIED_TC戻り、9=PROPAGATING戻り、10=ACKNOWLEDGED戻り |
| DIR-RSTP-FDB-v1 / TCM | 1=実FDB_COMPLETE応答でfdbFlushを解除する観測行。規範表の追加行とは称さない |

PSTの通常taken行はすべて専用fixtureで表し、fallbackを設けない。PRTの架空の行26とTCMの架空のFDB行13.39/11は受理しない。TCM行5はproducerがDETECTED戻りにだけ割り当て、他のUCTにはそれぞれの行を使う。

### 7.2 MVRPの専用fixture入力lookup

`guards.rs` の静的 `(DIR cell番号, fixture番号)` 配列は [mvrp-standard-v1.json](../fixtures/mvrp-standard-v1.json) の **id/kind/inputのみ**から抽出したmetadata。expectedのstate/actionからIDやFSM動作を決めず、fixture generatorも製品へ転用しない。製品実行時にfixtureファイルを開かない。元fixtureのSHA-256は配列直上のコメントに記録する。

対象は `fsm_applicant`、`fsm_applicant_admin`、`fsm_tx_capacity`、`fsm_registrar`、`fsm_registrar_admin`、`fsm_leaveall`、`fsm_periodic` の単一event入力。BeginはすべてBEGINに正準化する。その他のintegration、sequence、wire、action単独fixtureは、単一guard cellの専用IDとして流用しない。非Begin専用ID数はApplicant=752、Registrar=84、LeaveAll=6、Periodic=6。

省略入力は既存 `wire_codecs.rs` の実reducer刺激と同じsetup値に固定する：p2p/room=true、Registrar adminのsource=MT、capacityのpost-Registrar=MT。これらを全source/controlへwildcard拡張しない。Applicant matrixのregistrarは**Applicant評価時の実post-Registrar state**と照合する。実source、event、control、p2p、容量結果が異なる未被覆cellは、それ自身のfallbackを使う。Registrarの出力stateやApplicantの出力stateをID計算に使わない（Applicantへ渡すpost-Registrar入力だけは実分岐入力）。管理modeは各機械自身のmodeを使う。

### 7.3 DIR MVRP有限cell番号の固定方法

fallback sectionの `DIR-MRP-*-cells-v1` は**DIR独自の完全入力列挙**であり、IEEE Tables10-3/4/5/6に7776行等があるという意味ではない。設計§4–6の規則を使う各機械のguard identityを、下記の軸順で1始まりに凍結する。対応fixtureがあるcellとBegin cellのfallback文字列はcatalogに含めず、専用ID/BEGINだけを受理する。

| machine | 凍結section | 全cell番号の有限範囲 | 外側から内側への軸順 |
| --- | --- | --- | --- |
| Applicant | DIR-MRP-Applicant-cells-v1 | 1..7776 | source12 × event18 × applicant-control3 × post-Registrar-state3 × p2p2 × room2 |
| Registrar | DIR-MRP-Registrar-cells-v1 | 1..432 | source3 × event18 × registrar-control4 × p2p2 |
| LeaveAll | DIR-MRP-LeaveAll-cells-v1 | 1..8 | source2 × event4 |
| Periodic | DIR-MRP-Periodic-cells-v1 | 1..8 | source2 × event4 |

source順はApplicant=VO,VP,VN,AN,AA,QA,LA,AO,QO,AP,QP,LO、Registrar=IN,LV,MT、auxiliary=Active,Passive。event18順はBegin,New,Join,Lv,RNew,RJoinIn,RIn,RJoinMt,RMt,RLv,RLA,Redeclare,Periodic,Tx,TxLA,TxLAF,Flush,LeaveExpired。Applicant control順はnormal,new_only,non_participant。Registrar control順はnormal,fixed_ignore_new,fixed_propagate_new,forbidden。bool順はfalse,true。LeaveAll event順はBegin,ReceiveLeaveAll,LeaveAllExpired,Tx、PeriodicはBegin,PeriodicExpired,Enabled,Disabled。

番号はこれらの0始まりindexを外側からmixed-radixで畳み込み、最後に1を加える。例えばApplicant VO/New/normal/MT/true/trueはcell48で、専用fixtureがあるためfallbackではなくP-MVRP-T01-V305を出す。Begin以外では1cellに1IDを割り当て、異なるcellの専用ID/fallbackは重複しない。全入力空間にはno-op/未到達組合せも含むが、catalogの存在をその組合せの実到達性や公開journalの発生証明とはしない。

guard testsはfixture入力と実pure reducerのbefore/after、全8224 cellの有限lookup、重複・未知値・別機械・非正準ID、RSTP実migration/admin/auto-edge/UCT経路を検証する。専用test IDの対応や有限列挙の成功は、他の全条件、IEEE全体適合性、host/CLI/全workspaceの合格を証明しない。

## 8. 公開metadataとその他のwire record

`public_state_projection` を `network/wire.rs::wire_snapshot` の公開metadata境界で適用する。private runtime snapshotを直接変更しない。既知のRSTP bridge ID、全priority tuple、root_port、port counter、generations/tx_ordinalsは正準文字列へ変換する。packetのpriorityも対象。既に文字列であるMVRP timer/generation/ordinalとmembership時刻は維持する。既知のbounded u16/u32設定、VID/domainなどは既存の明示JSON整数契約を維持する。任意のJSON numberを再帰的に全て文字列化しない。

output validatorはtransitionだけでなくeventのeffect、timer/key、port policy、registration、MAC flush scope、policyのregistered/MAC、membership query/change/key/sourcesまでexact keyと型を検査する。nullableキーも省略不可。active drop理由は共通契約の有限enumを使い、codec内部の詳細エラー名を公開共通enumへ漏らさない。sorted uniqueのPortSet、VID/flush key、registration/MAC順を検査する。membershipのIP family、query整数幅、lease actionと時刻のnull関係も検査する。schema versionの変更や検証の迂回は行わない。

## 9. 検証境界

`runtime::wire::transition` のunit testsは全機械/state/変数型と負例、全文buffer射影、初期root、priorityの精度、metadataの境界、実port集合を検査する。`output::wire` のunit testsはnested effect/timer/policy/membershipの正例・負例を検査する。actual producer、guard identity、PRS観測の完全性、実CLI/JSON.parseと全体gateの結果は統合実行記録を参照する。validator単体の成功をproducerの成功やIEEE全体への適合性に読み替えない。
