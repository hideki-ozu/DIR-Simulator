# RSTP・MVRP-wire実装設計書

文書バージョン：`1.1.0`
<br>
対象GitHubバージョン：`v1.1.6`
<br>
文書ID：`proposal-implementation-bridge-wire`
<br>
文書状態：承認済み限定scopeの現行実装済み（v1.1.6収録）。本書は採用した詳細契約。製品試験の実施範囲・最終判定は別記録。

### 更新履歴

| 版 | 日付 | 内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-17 | 初版。規範本文に基づく能動参加scope、機械・codec補足、DIR統合・限定FDB flush、独立設計vectorを記録。v1.1.6収録版を確定 |

## 1. 要件

<a id="p-rstp-r01"></a>
**P-RSTP-R01**：能動bridgeがBPDU受信、情報比較、role/state遷移、timer満了、BPDU生成、topology changeに従って転送・学習可否を変更する。正常packetを抽象tree再計算の起動信号に置き換えない。対象は単一共通tree、全二重point-to-pointのcustomer bridge network。Root/Designated/Alternate/Backup/Disabledを扱う。規範closureに必要な旧STP Config/TCN互換とprotocol migrationを含める。shared LAN topology、MST region/MSTI、provider bridgeは初期対象外。管理edgeと自動判定の正確な設定・遷移はRSTP規範補足で定める。受信互換を単に無視して適合とは呼ばない。

<a id="p-rstp-r02"></a>
**P-RSTP-R02**：能動参加機械をDIRの時刻・queue・FES・policy・recordへ原子的に接続し、抽象モデルとの二重所有を拒否する。規範部分が未確定ならprepareを拒否し、部分的parserを能動実装として公開しない。

<a id="p-mvrp-r01"></a>
**P-MVRP-R01**：customer bridgeの各対象portがApplicantとRegistrarの両方を持ち、local declaration、PDU、timer、topology変更から登録・撤回・送信を決定する。MAPによるport間伝播も対象。VIDはDIR既存範囲1..4094。端末だけ、受動snoopingだけへのscope変更はしない。MMRP/MSRP、provider MVRP、GVRP互換は初期候補外。完全な選択roleの状態・全event・guard・action・timer・TX表が確定するまで設計完了にしない。

<a id="p-mvrp-r02"></a>
**P-MVRP-R02**：static VLAN許可とwire登録所有権を分け、複数attributeを含むPDUをall-or-nothingで処理し、queue再判定、timer失効、出力追跡を一貫させる。

## 2. 機能と完了判定

<a id="p-rstp-f01"></a>
**P-RSTP-F01 ← R01**：BPDU decode/encode、能動RSTP機械、選出、port state、送信/再送、TC処理。規範本文に基づく機械・codecは各規範補足を正とする。

<a id="p-rstp-f02"></a>
**P-RSTP-F02 ← R02**：入力検査、adapter所有、port stateの独立表現、phase/timer、NetworkDelta、queueと観測の原子commit。

<a id="p-mvrp-f01"></a>
**P-MVRP-F01 ← R01**：MRP vector decode/encode、Applicant/Registrar/LeaveAll/Periodicの機械、MAP、送信pack。規範本文に基づく機械・codecは各規範補足を正とする。

<a id="p-mvrp-f02"></a>
**P-MVRP-F02 ← R02**：wire registration投影、静的許可合成、全PDU検査、generation timer、queue再判定、record。

## 3. 詳細仕様

<a id="p-rstp-s01"></a>
**P-RSTP-S01 ← F01**：採用設計対象版を **IEEE Std 802.1Q-2022（base edition）** に固定する。最新への暗黙追従、旧802.1D版との混成はしない。原文の条項・図表と採用範囲を規範補足に固定した。製品revisionの受理は実装・製品検証後に登録する。

<a id="p-mvrp-s01"></a>
**P-MVRP-S01 ← F01**：同じ802.1Q-2022を対象に、MRPとMVRPのcodec・機械を一組として照合する。802.1ak draftで穴埋めしない。

### 3.1. 採用一次資料

2026-10-17にIEEE GETから[IEEE Std 802.1Q-2022](https://ieeexplore.ieee.org/document/10004498)の正規PDFを取得した。[公式版案内](https://standards.ieee.org/ieee/802.1Q/10323/)と版・標題を照合済み。2163 PDF pages、20,996,920 bytes、SHA-256 `1653580bcae81ed4753ff2840b40d67239036078ed550cff23325d6fdd697114`。

原文と作業用抽出は元リポジトリの`tmp/standards/`だけに保存し、既存`.gitignore`の`tmp/`でGit管理対象外とする。認証情報・ライセンス利用者情報を公開文書へ含めない。[取得来歴](../verification/ieee-8021q-source-2026-10-17.json)は版・hashと参照範囲だけを保持する。

| 規範範囲 | 本文で確定する内容 |
| --- | --- |
| Clause 10 | MRP Applicant/Registrar/LeaveAll/Periodic、event/action/timer、MRPDU packing/parsing |
| Clause 11.2 | MVRP application、VID/PVID、登録サービス、MAP、管理制御 |
| Clause 13 | 選択したRSTP/CISTのpriority、procedure、port/bridge機械、migrationとTC |
| Clause 14 | BPDUのfield、encoding、受信validationと互換性 |

採用は2022 base editionに固定する。IEEE SAの関連文書一覧とMaintenance TGを照合した範囲では、別途公開された802.1Q-2022 corrigendumは確認できなかった。後続amendment、保守draft、会議資料の修正を暗黙適用しない。規範補足と独立レビューにより、採用した限定scopeの設計gateを閉じた。後続改訂の採用・全IEEE適合を証明するものではない。

### 3.2. DIR設定契約

<a id="p-rstp-s02"></a>
**P-RSTP-S02 ← F02**：以下はDIR固有の新規提案設定。既存P-WIRE入口と互換に詳細化する。未知fieldは全階層で拒否し、nullを省略と同一視しない。未実装revisionは常にprepare拒否。

<a id="p-mvrp-s02"></a>
**P-MVRP-S02 ← F02**：同じ入口を使い、protocol別state所有範囲を静的検査する。別protocol同士の同一port利用は許すが、自動profile合成は追加しない。

| field | 型・範囲・単位 | 省略/default/0・異常 |
| --- | --- | --- |
| `protocol` | `rstp` / `mvrp` | 必須、未知値拒否 |
| `protocol_revision` | `ieee802.1q-2022.rstp.initial.v1` / `ieee802.1q-2022.mvrp.initial.v1`（DIR設計識別子） | 必須、protocol一致必須。設計は固定済み、製品登録は未実施 |
| `ports` | 非空の既存output port ID配列 | 必須、重複・非switch・未接続・全二重p2p以外を拒否。ingressから対応output portへのpairはprepared topologyで一意 |
| `max_pdu_bytes` | JSON正整数u32、byte | 必須、0拒否。PDUとはMAC client bytes。物理frame上限を別途検査 |
| `max_attributes_per_pdu` | JSON整数1..4094 | MVRP必須。RSTPではfield自体拒否。重複もdecode順に上限計数 |
| `max_state_entries` | JSON正整数u32 | 必須。RSTP port数、MVRPのport×対象VID数をprepareでchecked計算し上限内を要求 |
| `max_pending_timers`, `max_pending_tx` | JSON正整数u32 | 必須。上限はprotocol instance単位。0拒否 |
| `max_control_steps` | JSON正整数u32 | 必須。1callback内機械安定化上限、超過はrun失敗（途中commit禁止） |
| `local_declarations` | `{port,vid,declared}`配列。vid=JSON整数1..4094、declared=bool | MVRPのみ任意、default=[]。重複key拒否。static許可の有無とは別のlocal需要 |
| registrableの参照 | top-level `ethernet.dynamic.registrable`を正本とする | adapter内に同名fieldを置かない。static VLANも宣言可能で撤回によりstaticを消さない |
| `rstp` | RSTP専用object、必須 | [RSTP補足 §2](RSTP規範仕様・詳細設計書.md)のbridge/port ID、timer、管理flagとdefault。MVRPではfield自体拒否 |
| `port_parameters`, `registrar_controls`, `random_seed` | MVRP専用、任意 | [MVRP補足 §2](MVRP規範仕様・詳細設計書.md)の正確な型・default・範囲。RSTPではfield自体拒否 |

既存dynamicの`bridge_id:u64`とwire bridge identifierは別の入力で、暗黙変換しない。`ethernet.dynamic.convergence_ps`はrun共通の必須scalarである。同runにwire RSTPが一つでもあれば`"0"`固定、wire ownerでは未使用、併存するabstract領域も0を使う。wire RSTPがないrunだけ従来値を許す。RSTP adapter/rstp object内の同名fieldは拒否。wire RSTP領域では`tree()`と収束timerの生成を停止する。領域境界にabstract treeのbridgeを混在させず、同一連結領域の全bridge portを同じownerへ固定する。`link_set`は物理状態イベントなので許可するが、abstract tree再計算に渡さない。MVRP対象port/VIDへのabstract `vlan_register` / `vlan_unregister`はprepare拒否。他のmembership操作は所有stateが違うのでこの規則だけでは拒否しない。

### 3.3. packet/時刻/異常

設定rootとadapter/packet unionは[共通仕様](../後続拡張契約提案仕様書.md#wire)を正とする。packetは必須6field `id/adapter_id/ingress_port/arrival_ps/encapsulation/bytes_hex`。encapsulationは`mac_frame_fcs`固定、revisionはadapter_idから取得しpacket内重複指定を拒否する。`arrival_ps`は正準u64十進文字列（`0`可、先頭0、符号、空白、小数、overflow拒否）。packetのIDは入力に明示する非空UTF-8でrun内一意。内部生成TX IDは`@wire/tx/<adapter_id>/<ordinal>`としchecked u64 ordinalで重複を防ぐ。`bytes_hex`はMAC DAからFCSまでの完全なMAC frame、preamble/SFD/IFGを含まない。偶数桁ASCII hex、大小文字受理、出力lowercase。空hexはprepare拒否。これはP-WIREのbyte境界を明示する追加判断であり、PDU payloadだけを入力していた外部試案とは互換でない。

入力fixtureはpeerが送信完了したframeをportへ到着させる外部刺激である。到着時刻から逆算して存在しないpeer queueを作らず、観測には`external_injection`を明記する。内部adapterのTXは必ず実際の方向queue/帯域を占有する。既知hex構文・サイズ・未知revision・port不整合はprepare失敗。正しいhex内の不正MAC/FCS/PDUは実受信同様control_drop（prepare時に規範内容を先取り実行しない）。`arrival_ps>=T`は[0,T)外で未実行のまま保持。

動的受信はport mapping→link up→frame bounds/FCS→adapter demux→protocol全体検査→state machine→delta preflightの順。不正frame/PDUで一部attribute、timer、TXを残さない。未知protocolはcontrol_dropする。受理済protocolのwire version/type/reserved fieldのvalidation、ignore、互換処理は各規範補足に従う。一律unknown-version dropで標準の互換条件を上書きしない。通常data frameは既存data経路。規範のfield別ignore・互換受信は各補足のcodec規則を優先する。

## 4. architecture責務

<a id="p-rstp-a01"></a>
**P-RSTP-A01 ← S01**：`RstpAdapter`は規範codecと全port/bridge機械を所有。FES/queue/Registryを直接変更しない。候補関数は`decode(frame)->ValidatedPdu`、`step(snapshot,event)->ProtocolDelta`、`encode(tx_intent)->FrameBytes`。codecとstepの規則はRSTP補足に固定し、DIR側のcommitはcoordinatorが担う。

<a id="p-mvrp-a01"></a>
**P-MVRP-A01 ← S01**：`MvrpAdapter`はport×VIDのApplicant/Registrar、port timer群、MAP、PDU packを所有。機械が出す登録投影とwire送信は独立のeffectであり「登録したら毎回Join送信」という近道は禁止。

<a id="p-rstp-a02"></a>
**P-RSTP-A02 ← S02**：`NetworkRuntime`がport対、FES、実TX、queue、NetworkDeltaのcommitを所有。`BridgePolicyProjection`がroleとdata learning/forwardingを別々に公開。`DynamicState`は既存abstract ownerのときだけ中央treeを計算する。

<a id="p-mvrp-a02"></a>
**P-MVRP-A02 ← S02**：`WireRegistrationProjection`がactive登録集合を所有し、`DynamicState`のstatic/effective VLAN合成に一方向で反映する。抽象leaseを擬似的な無限期限として流用しない。出力層は新schemaでこの根拠を再生する。

| 現行接続先（基準source） | 確認した挙動 | 将来変更責務 |
| --- | --- | --- |
| `input/ethernet/dynamic.rs::prepare` | 全switch ID、registrable、正整数上限、VIDを検査 | owner modeでwire設定を別検査。既存入力は不変 |
| `lib/types/ethernet/dynamic.rs::PortRole` | Root/Designatedならforwarding、Learning stateなし | wire policy V2でroleとstate分離。既存enumを都合よく再定義しない |
| `runtime/ethernet/dynamic.rs::tree, plan_controls` | 中央最小cost tree、link変化で全MAC flush、convergence後publish | wire RSTP領域から呼ばない。TCのflush対象は規範機械の明示effectのみ |
| 同 `Lease/Partition` | registrationはexpires_at必須 | wire集合は別所有、寿命はMRP機械のtimerから明示撤回 |
| `runtime/network.rs::arbitrate` | queue全待機copyをeligibilityでdrop、activeは別 | wire control laneをdata filterから分離、dataは最新projection使用 |
| 同 `NetworkDelta`, `emit_dynamic`, `apply` | staged policyとrecord、dirty ports、commit | protocol state/timer/TXも同一deltaへ追加 |
| `runtime/ethernet.rs::serialize_wire` | ether_type<1536、予約DAを拒否 | typed control serializer追加。通常dataの制限は緩めない |
| `lib/snapshot/ethernet/dynamic.rs`, `output/network.rs` | control/policy差分、schema2 model_records | 新wire記録を明示登録。既存kindに偽装しない |
| `tool/viewer/assets/ethernet-model.js` | 許可field/kind・policy再生を厳密検査 | V2を別dispatcherで読む。legacy viewerに無理に流さない |

パスの先頭は全て `crates/dir-simulator/src/`。既存変更中のTimer APIを再定義しない。model Contextの予約/commitとcurrent timer token検査を使い、raw FESへの抜け道を作らない。

## 5. 詳細設計

<a id="p-rstp-d01"></a>
**P-RSTP-D01 ← A01**：[RSTP規範仕様・詳細設計書](RSTP規範仕様・詳細設計書.md)を正とする。選択した10機械・64状態、guard優先、entry action、timer、helper、安定化、旧STP互換、完全MAC frame codecを固定した。166設計vectorsを持つ。Superior/Repeatedの本文優先、role0の規範本文と参考NOTEの優先、tick丸めは同書§12へ明記した。現行の抽象tree計算へ委譲しない。

<a id="p-mvrp-d01"></a>
**P-MVRP-D01 ← A01**：[MVRP規範仕様・詳細設計書](MVRP規範仕様・詳細設計書.md)を正とする。Full Participant、MAP、管理、timer、TX容量、base6 packing、VID0→PVID、未来版の構造的skipと正常/異常byteを固定した。981設計vectorsを持つ。LeaveAllはlocal rLAを省略せず、同書§6のDIR決定順序でtxLA/txLAFまで実行する。

<a id="p-rstp-d02"></a>
**P-RSTP-D02 ← A02**：以下5.1–5.7をRSTPのDIR接続契約とする。

<a id="p-mvrp-d02"></a>
**P-MVRP-D02 ← A02**：同じ接続契約と5.2登録投影をMVRPへ適用する。規範上の判断とDIR固有処理の境界を維持する。

### 5.1. 時刻、状態所有、timer

初期host状態は`prepared`、run開始phase0で`running`、半開区間末尾で`suspended`、preflight失敗で`failed`。`prepared`から開始前はpacket受信不可。`running`以外でcallback実行不可。resumeはrun engine所有で、protocolが独自に時計を進めない。これはDIR host lifecycleであり、標準機械の代用品ではない。

各callbackはsnapshotを読み、PDU全体を検査してからprotocol機械の局所copyを進め、同deltaの先行effectを後続判断に可視化する。timer keyは`(protocol,bridge,port,slot,attribute)`、attributeはMVRP VID又はnull。slot名と標準期間は各規範補足のtimer表を正とする。RSTPは1秒tickで残秒counterを減らし、wireの1/256秒値をそのまま独立deadlineへ置き換えない。keyの世代u64は取消し後もtombstoneを保持し、再利用で古いtimerを復活させない。arm/rearm/cancelでchecked +1、初期0から最初1。未armのcancelはno-op。payloadはkey/token/deadline。pending上限は生きたkeyだけに適用し、stale eventの物理FES容量はengine側上限にも算入する。

`deadline=now+duration`はchecked u64。DIR adapterのtimer intentではduration=0を拒否し、即時遷移は同callback内の安定化で表現する（標準timerが零値を要求する場合も即時eventに正規化する）。deadline==Tは実行せずpending。timer満了はphase0、packet受信はphase1、egress選択はphase2。同phaseは既存決定的event順。phase1で得たpolicy変更は同callbackでcommitし、その時刻のphase2に見える。phase0へ遡及予約しない。

満了と受信が同時なら満了が先。例えば旧登録timer deadline=100、再登録packet arrival=100では満了のcommit後に受信処理となる。既存abstract `plan_controls`は同時刻workload更新をexpiryより先に見るが、wire packetをこの抽象更新配列へ注入しない。これが意図した境界差である。

### 5.2. policy投影とqueue

wire policy V2はportごとに`role`（`Root/Designated/Alternate/Backup/Disabled`）、`learning:bool`、`forwarding:bool`、`link_up:bool`を保持。DIRで許すboolean組は(false,false)、(true,false)、(true,true)。false,trueはpreflight不変条件違反。初期は全wire portで両方false、roleは規範初期化から供給（捏造しない）。roleだけからforwardingを導出しない。受信data学習はlearning時のみ、data relayとegressはforwarding時のみ。

MVRP投影は`registered:Set<(port,vid)>`、`registration_generation:Map<key,u64>`。各set変更は世代+1、同値setはno-op、remove済みkeyも世代を保持。static許可集合S、wire登録集合W、許可表Aとしてeffective許可は `S ∪ (W ∩ A)`。tagged値はS優先、W側はAの値を使う。MVRPのlocal declarationは別集合で、単独ではregisteredをtrueにしない。active timerを偽のexpires_at=U64_MAXへ変換しない。

data egress不適格理由の優先はlink_down→stp_learning（learning=true/forwarding=false）又はstp_discarding（両false）→vlan_unregistered。変更したportをdirtyにしphase2を要求。全waiting copyを現行同様再判定し不適格ならdrop、active copyは中断せずEOF/releaseまで進める。受信側は到着時policyで再判定する。このin-flight規則はDIR抽象媒体制約。protocol state機械の規範timerと同一ではない。wire control TX/RXはdataのforwarding/VID filterを通さず、物理link downだけを適用する。

不正なadapter effect（未宣言port、静的許可自体の削除要求、許可外effective policyへの直接変更）は内部不変条件違反としてrun失敗、batch全棄却とする。MVRPの規範Registrar集合Wは全VID1..4094を扱い、DIRの許可集合AにないVIDも規範に従って登録できる。AはPDU受理条件に使わず、effective投影S∪(W∩A)にだけ適用する。Restricted_VLAN_Registrationの規範管理制御とは別の責務である。

policy投影が変化したcallbackでpolicy_epochを一回checked +1。protocol内部だけ変化した場合はpolicy_epoch不変、protocol_generationを一回+1。topology_generationはlink/role/learning/forwardingの可視変更時一回+1、registrationだけでは不変。重複PDUはcodecが同じでも規範機械がtimerをrefreshし得るのでbyte重複だけでno-opにしない。

### 5.3. TXと原子性

adapter TX intentはport、canonical frame bytes、source event、ordinalを持つ。codecが完全frameを作り、MAC size/FCS検証後にcontrol専用FIFOへ入れる。FIFOはport内でcallback順/ordinal順、同一frameの同時送信複製はしない。phase2で現在送信が空ならcontrolをdataより先に選択し、開始済みdataはpreemptしない。controlにはdata gate/credit/VID filterを掛けない。初期候補ではTSN/preemption profileとの合成をprepare拒否し、この優先規則をIEEEの一般適合と呼ばない。

内部生成packetは通常transferと同じ帯域、EOF、IFG、arrivalイベントを持つ。controlのdropはdata要求成功率の分母へ入れず別counterで記録する。送信開始後にlink downとなっても現在の媒体占有は完了、peer到着時link downならcontrol_drop。規範機械の送信actionはcontrol FIFOへのacceptを送信要求の完了とし、媒体EOFは観測eventとする。FIFO予約失敗は全callback失敗で、送信counterだけを先行更新しない。

preflight対象は全decode結果、referential integrity、機械安定化上限、timer deadline/token、protocol/policy世代、全queue容量、record容量、FES予約、packet ID採番、算術。全部成功後にstate/TX/timer/recordを一括commit。capacity不足/算術overflowはrun失敗としcontrol_dropへ読み替えない。当該callbackの全effectは0、成功prefixは保持。bad PDUは正常なdrop recordのみcommitし、protocol/policy/timer/TXは不変。stale timerもstale recordのみ。error recordの確保自体が失敗したらrun diagnosticのみを残す。

### 5.4. 共通出力schema

`simulation.model_records`に新kind `ethernet.control.wire.event`を提案する。既存record共通外枠を維持し`request_id=null`、`record_id=@wire/event/<cursor>`。cursorはcommit時のみcheckedで進める。全field必須、nullableは下記だけ。u64はJSON正準十進文字列、u32はJSON整数。

| data field | 型・意味 |
| --- | --- |
| `schema_version` | JSON整数1（この新event内schema） |
| `protocol`, `protocol_revision` | 3.2の列挙/識別子 |
| `bridge`, `port` | prepared ID string |
| `event_id`, `cause_id` | event ID string、causeはpacket/timer/link/local event ID string又は開始時null |
| `event_kind` | `initialize/receive/timer/link/local/tx_queued/tx_started/tx_completed` |
| `time_ps`, `phase`, `batch_ordinal` | u64、JSON整数0..2、u64 |
| `packet_id`, `bytes_hex`, `mac_bytes` | packet関連eventはstring/string/u32、それ以外は全てnull。hexはDA..FCS |
| `outcome` | `changed/no_op/control_drop/stale` |
| `reason` | 正常null。drop=`link_down/frame_bounds/fcs/protocol_malformed/unsupported_revision/unsupported_kind`、stale=`timer_token` |
| `policy_epoch_before`, `policy_epoch_after` | u64 |
| `protocol_generation_before`, `protocol_generation_after` | u64 |
| `timer` | null又は`{key,token,deadline_ps}`。key=`{protocol,bridge,port,slot,attribute}`、attributeはJSON VID又はnull |
| `effects` | 下記effect配列、順序を保持。drop/staleは[] |
| `external_injection` | bool。外部packet受信だけtrue |

effectはtagged union。mac_flushは5.7の完全fieldを正とする。`port_policy`は`{kind,port,before,after}`（5.2の4field）、`registration`は`{kind,port,vid,before,after,generation}`（before/after bool）、`timer_arm/timer_cancel`は`{kind,key,token,deadline_ps}`（cancelのdeadlineはnull）、`tx`は`{kind,packet_id,port,ordinal}`。MAC flushは5.6でcoordinatorが対象を解決し、5.7の完了effectにする。未知effect/field拒否。

別kind `ethernet.control.wire.policy`は`{schema_version:1,time_ps,policy_epoch,topology_generation,ports,registered,mac}`。portsはID順で5.2の4fieldを持つmap、registeredはport/VID順の`{port,vid,generation}`配列。初期snapshot必須。以降policy changeごとに一件出力。effective VLANは上記集合式で再生、既存static入力とregistrable入力を参照する。全規範transitionは5.7の別kindに記録する。このpolicy snapshotとeventだけで内部機械stateを推測しない。

### 5.5. 実装順と受け渡しgate

1. 完了した規範補足、採用版解釈、DIR統合境界、独立設計vectorを実装の基準として凍結する。採用版を変更する場合は差分条項を再設計・再レビューする。
2. 現行profileを変えずcontrol frame型、V2 policy、wire所有state、共通plannerと記録dispatcherを追加。既存abstract入力/出力の回帰を確認。
3. protocol機械をpure stepとして実装し全遷移coverageを検査。decodeとencodeの相互一致だけを独立oracleと呼ばない。
4. 単一port、二bridge、triangle、link flap、同時timer/PDU、queued/active data、複数VID、容量失敗を統合試験。wire TXをpcap等の独立手段で照合。
5. 未解決標準判断が0、全選択roleのclosureが確認できた場合のみrevisionを承認。正式採用工程でRegistry、正式要件、Viewer schemaを追加する。本設計作業では追加しない。

<a id="bridge-learning-flush"></a>
### 5.6. 共通SVRE・MAC学習・限定flush

この節は既存`runtime/ethernet/dynamic.rs::Partition<MacKey,String>`と`plan_learning`をwire profileへ結合する設計である。通常dynamic profileの「link変更時に全dynamic MACを消す」経路をwire RSTP/MVRPへ流用しない。初期wireでも既存のMAC学習・aging・静的FDB優先を維持する。

**SVREの唯一の所有者**はprepareが作るbridge共通catalog。規範のStatic VLAN Registration EntryはVID単位entryと全outbound portのMapである（§8.8.2）。内部は`svre_exists[bridge,vid]`、`registrar_mode[bridge,port,vid]`、既存S/A/tag参照で表す。Sのkeyはfixed_ignore_new、SにないAのkeyはNormalへ正規化し、S/Aのいずれかのportが当該VIDを持てばbridge/VID entryを作る。他のportはNormal既定。MVRP.registrar_controlsの明示rowもこのcatalogへの入力変換とし、そのVIDのentryを作る。S上Normal/ForbiddenとS外Fixedは初期設計ではprepare拒否、S上Fixedのignore/propagate選択、S外Normal/Forbiddenは許す。これは既存DIRの常時membershipを守るmappingで、IEEEがS/Aという入力形式を定めたという意味ではない。catalogはprepare後不変で、MVRP無しでもRSTPがS由来Fixedを参照できる。

Restricted_VLAN_Registrationがtrueなら、`svre_exists[bridge,vid] && mode[port,vid]==Normal`だけが動的登録対象。対象portの明示rowは必須ではなく、別portのS/A/明示rowからentryが存在する場合も判定する。A外でも規範Wは登録でき、data許可は常に`S ∪ (W ∩ A)`。Fixedの初期source、Restrictedの判定、RSTPの固定VID maskは同じcatalogから導出する。static MAC FDB entryはSVREではない。

**data ingress**はFCS/admit/link/VIDの可否を求め、individual source MACに対し`learning=true`のportで既存learn/refresh/moveを行う。その後`forwarding=false`ならrelayしない。Learning状態では学習だけ行い`stp_learning`でrelayを止め、Discardingでは学習も行わない。制御BPDU/MVRPDUは専用consume経路で、MAC学習を行わない。lookupはstatic→有効dynamic→unknown flood、known egressが不適格ならfloodへ変えない。MAC満杯は既存mac_capacity監査＋frame処理継続、同key更新は容量満杯でも可能。source ledgerは既存prepare走査を再利用する。

**RSTP flush**は、当該bridgeの最新staged operEdgeから`fixedVIDmask={vid | 非edge portのいずれかが当該VIDのFixed}`を求める。link_up／forwardingを追加条件にしない。要求元portがedgeなら削除0。非edgeなら、そのportを学習先に持ちVIDがmask内のdynamic entryだけを削除する。static MAC、別port、mask外VIDを保持する（§13.27.28 PDF554）。ForceProtocolVersion=2固定なので、legacy互換送信中も即時削除でありrapid ageingへ変えない。

**MVRP New flush**はMAD Join indicationとrequestの双方で、受け取るport/VIDのdynamic entryだけを削除する。RSTP固定VID maskとoperEdgeによる削除省略をここへ流用しない。static MACと他VID/portは保持する（§11.2.5 PDF322）。

flush要求の処理では、削除keyを`(switch,vid,mac)`順に列挙し、entryとdeadline indexを同じMacDeltaから除く。generation ledgerは保持し、再学習は前世代+1。古い期限はlive entry不存在又は世代不一致で無効果とし、新entryを消さない。coordinatorの次wakeを再計算し、FES cancel/armと監査も同じbatchへ含める。

FDB完了応答はstaged MacDeltaで削除が確定する見込みのsnapshotに対して返す。RSTPはそこでfdbFlush=falseとして安定化を継続する。空flushでも要求と完了を記録する。最終preflightに失敗したらprotocol状態・削除・完了応答・timer・record全て未commit。実FDB削除前にlive RSTP flagだけをclearしない。

<a id="bridge-protocol-order"></a>
### 5.7. protocol間の因果順序と観測

各adapterは一bridgeを所有し、same protocolの別bridge instanceを許す。内部frameのpeer到着はprotocolとingress bridgeから受信ownerを一意に選ぶ。RSTP未対応peerと未対応endpointは制御frameを消費・dropする。MVRPは対象の物理連結領域内の全bridgeにinstanceと全接続portの所有をprepareで要求する。未対応bridge経由のMVRP relay（§10.8.3.2(b)、PDF304）は初期対象外として混在配置を拒否し、run中にowner不在bridgeへ到達したら内部不変条件違反とする。port_identitiesがcontrol TX SAの唯一の正本である。

coordinatorの一callbackはcommitted snapshotを一度読み、同じDeltaの先行差分を参照する。RSTPのrole/state機械と必要なFDB完了応答を安定化し、TCM DETECTEDのnewTcDetected呼出しからMRP contextのtcDetectedを更新する。受信TCや非0tcWhileだけからtcDetected eventを捏造しない。

その後、(1) Root/Alternate→Designated portのMAD Registrar FlushとLeaveAll timer通知、(2) Designated→Root/AlternateのRe-declare、(3) 実際のForwarding集合の削除/追加、(4) MVRP MAP需要とnew通知の再評価、(5) MVRP機械・New flush・timer/TX intentの安定化、の順で処理する。MAPは同bridge/context0に限定し、伝播元port/contextのtcDetectedを見てnewを昇格する。MAPが作った需要を外部sourceへ戻さず、通知の再帰loopを作らない。複数bridge/protocolを同時刻に初期化する際はbridge ID→RSTP→MVRP→IGMP→MLDの固定順。FES phase規則を変更しない。

公開変数の型・機械別有限catalog・PRS観測scopeは[wire遷移公開catalog](wire遷移公開catalog.md)を正本とする。

全protocol・MAC差分が確定した後に一回だけpolicy epochを進め、queue再判定へ渡す。protocol内部stateだけの更新はadapterごとのprotocol_generationだけを進める。TX intentはこのcallback内の決定順でcontrol FIFOへ入れ、実送信はphase2で行う。workerのthreadや別の時計は作らない。

共通eventに`adapter_id`を追加する。`mac_flush` effectは必須`{kind,reason,port,vids,keys,completed}`、kind固定mac_flush、reason=`rstp_topology_change`/`mvrp_new`、vidsは昇順u16配列、keysは上記順の`{switch,vid,mac}`配列、completedはtrue。空keysも完了監査として出す。MACのlearn/move/expire/flushは既存dynamic record構造を再利用し、wireの同epochへ結び付ける。wire policy snapshotには必須mac配列`{switch,vid,mac,port,expires_at_ps,generation}`を追加し、空表も[]で出す。

規範transitionの完全記録は別kind `ethernet.control.wire.transition` version1。必須fieldは`schema_version,adapter_id,bridge,port,protocol,time_ps,callback_ordinal,transition_ordinal,machine,from_state,to_state,trigger,changes`。portはbridge機械だけnull、時刻/ordinalは十進u64文字列。machine/stateは分野の列挙名、triggerは受信packet ID／timer key／link ID／初期化又は固定guard識別子。changesはその機械のvariable catalogからの名前とbefore/after typed値。型はbool、u64十進、enum文字列、priority tuple整数文字列配列、Times field object、port-ID集合に限定する。未知variableと型違いを出力validatorで拒否する。機械単体guardの識別は分野fixture IDを使い、まだ専用caseのない表行は`<machine>/rule/<table-section>/<row-index>`（1始まり、初版表の行順）を固定キーとする。表示文言を変更してもキーを再採番しない。transient/self-loopを省略しない。

公開`simulation.model_records[].data`では、上記domain payloadにnetwork host共通の`effect_seq`（必須・正準十進u64文字列）を一つ追加する。hostはこの順序fieldを検証してからdomainの閉じたfield集合を検証し、未知fieldを除去して受理しない。adapter内のpure planner recordはhost付与前なので`effect_seq`を持たない。transitionのdomain本体は13field、公開carrierではこの1fieldを加えた14fieldとなる。

policy/transition/TX/flush/metricの全予約を同じNetworkDeltaで検査・commitする。容量やmax_control_steps超過はcallback失敗。配列の一部だけを公開したり通常packet dropへ読み替えたりしない。partial snapshotはこの成功prefixだけから再生し、planned timer/TXをactualへ補完しない。

## 6. 検証条件

<a id="p-rstp-t01"></a>
**P-RSTP-T01 ← D01**：全規範遷移と正確なBPDU正常/境界/異常byte、timer期限前/同時/後、proposal/agreement、TC、migrationの独立期待値が必要。[rstp-standard-v1.json](../fixtures/rstp-standard-v1.json)の166設計vectorsを参照。64 state entry、45 one-guard遷移を含むが、全guard組合せの製品試験ではない。

<a id="p-mvrp-t01"></a>
**P-MVRP-T01 ← D01**：全機械/MAP遷移、複数vector/attributeとpacking、無効count/terminator/VID、timer競合、送信byte、撤回と再宣言を独立検査。[mvrp-standard-v1.json](../fixtures/mvrp-standard-v1.json)の981設計vectorsを参照。全Applicant source/event軸、Registrar、LeaveAll、Periodic、管理、wireと容量境界を含む。

<a id="p-rstp-t02"></a>
**P-RSTP-T02 ← D02**：fixtureのframe byte上限/hex、timer stale/期限、capacity rollback、role/state分離、queue filtering、帯域算術を確認。媒体算術はDIR既存契約として、L=64 MAC byte、preamble/SFD=8 byte、IFG=12 byte、rate=100,000,000 bit/sならEOF差分5,760,000 ps、release差分6,720,000 ps。ceilは分子を広い整数で計算後u64範囲を検査。これはBPDU byte長を64と断定するものではない。

<a id="p-mvrp-t02"></a>
**P-MVRP-T02 ← D02**：static/wire集合の合成、unauthorized VID、全PDU rollback、timer token、unknown fieldの期待を確認。synthetic effect batchはcodecを通らないplanner単体のtest inputであり、PDU正常性を証明しない。

fixtureは上記規範2ファイルと[bridge-wire-v1.json](../fixtures/bridge-wire-v1.json)20件、[bridge-integration-v1.json](../fixtures/bridge-integration-v1.json)18件。後者は共通catalog/owner、Learning中の学習、port/VID限定flush、stale aging token、protocol因果順を扱う。全て`status=design_vectors_not_product_tests`。旧readinessの2件は「必要設計が欠けていた場合に公開gateを閉じる」仮想negativeで、現在の設計状態をfalseとする証拠ではない。旧hex構文の6件等は意味上の標準PDUではなく、別途正常/異常wireを規範fixtureへ追加した。CRC算術vectorのASCII `123456789`→CRC `cbf43926`は演算単体の確認で、正常MAC frameではない。再計算範囲と構造検査のみを[検証記録](../verification/実装準備設計検証記録.md)で区別する。

## 7. 提案追跡

以下は提案内の六工程と設計責務の追跡であり、製品実装・規範適合の証拠ではない。

```proposal-trace
{
  "nodes": [
    {
      "id": "P-RSTP-R01",
      "stage": "requirement",
      "upstream": [],
      "anchor": "p-rstp-r01"
    },
    {
      "id": "P-RSTP-F01",
      "stage": "function",
      "upstream": [
        "P-RSTP-R01"
      ],
      "anchor": "p-rstp-f01"
    },
    {
      "id": "P-RSTP-S01",
      "stage": "spec",
      "upstream": [
        "P-RSTP-F01"
      ],
      "anchor": "p-rstp-s01"
    },
    {
      "id": "P-RSTP-A01",
      "stage": "architecture",
      "upstream": [
        "P-RSTP-S01"
      ],
      "anchor": "p-rstp-a01"
    },
    {
      "id": "P-RSTP-D01",
      "stage": "design",
      "upstream": [
        "P-RSTP-A01"
      ],
      "anchor": "p-rstp-d01"
    },
    {
      "id": "P-RSTP-T01",
      "stage": "verification",
      "upstream": [
        "P-RSTP-D01"
      ],
      "anchor": "p-rstp-t01"
    },
    {
      "id": "P-RSTP-R02",
      "stage": "requirement",
      "upstream": [],
      "anchor": "p-rstp-r02"
    },
    {
      "id": "P-RSTP-F02",
      "stage": "function",
      "upstream": [
        "P-RSTP-R02"
      ],
      "anchor": "p-rstp-f02"
    },
    {
      "id": "P-RSTP-S02",
      "stage": "spec",
      "upstream": [
        "P-RSTP-F02"
      ],
      "anchor": "p-rstp-s02"
    },
    {
      "id": "P-RSTP-A02",
      "stage": "architecture",
      "upstream": [
        "P-RSTP-S02"
      ],
      "anchor": "p-rstp-a02"
    },
    {
      "id": "P-RSTP-D02",
      "stage": "design",
      "upstream": [
        "P-RSTP-A02"
      ],
      "anchor": "p-rstp-d02"
    },
    {
      "id": "P-RSTP-T02",
      "stage": "verification",
      "upstream": [
        "P-RSTP-D02"
      ],
      "anchor": "p-rstp-t02"
    },
    {
      "id": "P-MVRP-R01",
      "stage": "requirement",
      "upstream": [],
      "anchor": "p-mvrp-r01"
    },
    {
      "id": "P-MVRP-F01",
      "stage": "function",
      "upstream": [
        "P-MVRP-R01"
      ],
      "anchor": "p-mvrp-f01"
    },
    {
      "id": "P-MVRP-S01",
      "stage": "spec",
      "upstream": [
        "P-MVRP-F01"
      ],
      "anchor": "p-mvrp-s01"
    },
    {
      "id": "P-MVRP-A01",
      "stage": "architecture",
      "upstream": [
        "P-MVRP-S01"
      ],
      "anchor": "p-mvrp-a01"
    },
    {
      "id": "P-MVRP-D01",
      "stage": "design",
      "upstream": [
        "P-MVRP-A01"
      ],
      "anchor": "p-mvrp-d01"
    },
    {
      "id": "P-MVRP-T01",
      "stage": "verification",
      "upstream": [
        "P-MVRP-D01"
      ],
      "anchor": "p-mvrp-t01"
    },
    {
      "id": "P-MVRP-R02",
      "stage": "requirement",
      "upstream": [],
      "anchor": "p-mvrp-r02"
    },
    {
      "id": "P-MVRP-F02",
      "stage": "function",
      "upstream": [
        "P-MVRP-R02"
      ],
      "anchor": "p-mvrp-f02"
    },
    {
      "id": "P-MVRP-S02",
      "stage": "spec",
      "upstream": [
        "P-MVRP-F02"
      ],
      "anchor": "p-mvrp-s02"
    },
    {
      "id": "P-MVRP-A02",
      "stage": "architecture",
      "upstream": [
        "P-MVRP-S02"
      ],
      "anchor": "p-mvrp-a02"
    },
    {
      "id": "P-MVRP-D02",
      "stage": "design",
      "upstream": [
        "P-MVRP-A02"
      ],
      "anchor": "p-mvrp-d02"
    },
    {
      "id": "P-MVRP-T02",
      "stage": "verification",
      "upstream": [
        "P-MVRP-D02"
      ],
      "anchor": "p-mvrp-t02"
    }
  ]
}
```

2026-10-17採用作業記録：本書のP-IDを[正式採用仕様](../../specs/後続拡張採用詳細機能仕様書.md)へ接続した。[現行API・scope](../../guide/reference/後続10分野の現行実装と利用境界.md)と[製品検証記録](../../verification/results/extension-product-2026-10-17.md)を参照。設計vectorと過去のdesign-only報告は履歴のまま保持し、製品合格とは区別する。文書版・履歴は本pushの改訂として確定した。
