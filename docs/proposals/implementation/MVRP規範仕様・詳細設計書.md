# MVRP規範仕様・詳細設計書

文書バージョン：`1.1.0`

対象GitHubバージョン：`v1.1.6`

文書ID：`proposal-implementation-mvrp-normative`

文書状態：承認済み限定scopeの現行実装済み（v1.1.6収録）。本書は採用した詳細契約。製品試験の実施範囲・最終判定は別記録。

### 更新履歴

| 版 | 日付 | 内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-17 | IEEE 802.1Q-2022本文に照合した能動MVRP、独立状態刺激とwire期待値。v1.1.6収録版を確定 |

## 1. 範囲と根拠

[統合設計](RSTP・MVRP-wire実装設計書.md)と[共通wire契約](../後続拡張契約提案仕様書.md#wire)のMVRP規範機械を具体化する。対象はcustomer bridge、単一Base Spanning Tree Context (0)、各portにFull Participant (Applicant、Registrar、LeaveAll、PeriodicTransmission)を置く。実行topologyは全二重point-to-point。full Applicantの共有媒体側遷移も定義し、媒体構成を追加した際に状態機械を作り直さない。MSTI、provider、MMRP、MSRP、GVRP、VID translation tableは対象外で、設定として受理しない。受動snoopingへの縮小ではない。

参照本文はIEEE Std 802.1Q-2022、2163 PDFページ、SHA-256 `1653580bcae81ed4753ff2840b40d67239036078ed550cff23325d6fdd697114`。以下のPDF頁は1始まりで、印刷頁はPDF頁−1。標準原文、図表の画像、抽出本文は本書やfixtureに含めない。状態記号とfunctionalな条件・演算を独自の実装規則へ展開した。

| 対象 | 条項・表 | PDF頁 |
| --- | --- | --- |
| Participant、MAC operational、MAP | 10.1–10.3.1.1 | 275–279 |
| DA / EtherType | 10.5、Tables 10-1/10-2 | 280 |
| 管理、timer、event/action | 10.7.1–10.7.6 | 286–293 |
| Applicant / Registrar / LeaveAll | 10.7.7–10.7.9、Tables 10-3/4/5 | 293–295 |
| Periodic、timer値、TX間隔 | 10.7.10–10.7.11、Tables 10-6/7 | 296–297 |
| wire、packing、未知版、破損 | 10.8.1–10.8.3 | 299–304 |
| MVRP固有VID、PVID、登録、new | 11.2.3–11.2.7 | 319–322 |

## 2. 構成型と初期化

adapterの共通必須fieldは統合設計を継承する。`protocol=mvrp`、`protocol_revision=ieee802.1q-2022.mvrp.initial.v1`。revisionの製品受理は実装・検証後であり、この設計文書だけで製品prepareを有効化しない。全objectは未知field拒否、整数にbool/小数を受理しない。`ports`は一bridge内のswitch output port ID、ingressはprepared topologyで対応付ける。同protocol/bridgeにつき一instance、run全体では同protocolの複数bridge adapterを許す。bridge所有者はportsから一意に導出し、跨bridge配列はprepare拒否する。初期scopeではMVRPを配置した物理連結領域の全bridgeにinstanceを必須とし、全接続portを所有する。非対応bridgeのForwardingProcessへの引渡し（10.8.3.2(b)、PDF304）は初期未対応のため、未対応bridge混在・部分port配置をprepare拒否する。標準が非対応bridgeでのdropを要求するという意味ではない。

| adapter追加field | 正確な型、既定、省略と0 |
| --- | --- |
| `port_parameters` | 任意配列、default `[]`。要素は下表のobject。同一port重複、adapter外port拒否。省略portは全default |
| `registrar_controls` | 任意配列、default `[]`。要素は必須`{port:string,vid:integer,mode:string}`のみ。VID1..4094、key重複拒否。mode=`normal`/`fixed_ignore_new`/`fixed_propagate_new`/`forbidden`、省略keyは下記SVRE catalogから解決 |
| `local_declarations` | 任意、default `[]`。必須`{port:string,vid:integer,declared:bool}`のみ。VID1..4094、key重複拒否。初期trueはNew要求、falseは需要なし |
| `random_seed` | 正準u64十進文字列、default `"0"`、0可。負数・先頭0・空白・overflow拒否 |

`port_parameters`の要素は`port`だけ必須で、他fieldは省略可。管理変更は初期設定と後述の機械単体eventで仕様化する。今回の共通packet入力に未定義の管理commandを追加しない。

| port field | 型・default・範囲 |
| --- | --- |
| `port` | 既存port ID文字列 |
| `join_time_cs` | JSON整数、default20、1..1844674407、0拒否 |
| `leave_time_cs` | JSON整数、default60、1..1844674407、0拒否。標準default範囲60–100csから60を選択 |
| `leaveall_time_cs` | JSON整数、default1000、0..1229782938。0は自発LeaveAll timer停止、受信LeaveAll処理は残る |
| `periodic_enabled` | bool、default true。周期は固定1秒、周期長fieldは受理しない |
| `applicant_control` | enum `normal`/`new_only`/`non_participant`、default normal |
| `restricted_vlan_registration` | bool、default false |
| `transmit_zero` | bool、default false。真かつ当該portをuntagged setに含むVIDがちょうど一つのときだけそのVIDをwire0として送る |

`operPointToPointMAC`はprepared全二重接続からtrueを得る。独立FSM試験ではfalseも刺激できるが、新しいpublic boolean overrideは作らない。PVIDとuntagged setは既存Ethernet設定を参照する。PVIDは1..4094必須。`ethernet.dynamic.registrable`が許可集合Aの唯一の正本で、adapterに同じ集合を重複設定しない。Registrar管理はprotocol stateの管理でありAと同義ではない。prepareで単一のprotocol用Static VLAN Registration Entry (SVRE) catalogを正規化する。Sのkeyはfixed_ignore_new、Sに無いAのkeyは明示normal entry、S/Aのどちらにも無いport/VID keyの管理値はprotocol既定normalとする。SVRE entry存在性は後述のbridge/VID単位で別に求める。その後registrar_controls明示rowを重ねる。S上のnormal/forbiddenは常時membershipと矛盾するためprepare拒否し、S上fixed_ignore_new/fixed_propagate_newの選択は許す。S外の明示normal/forbiddenは許すが、S外fixed_*は常時membershipとの不一致になるためprepare拒否する。SVRE entryはport単位ではなくbridge/VID単位に存在し、`exists[bridge,vid]`をS/A/registrar_controlsのいずれかのportにrowがあることから求める。entry内の欠損port mapはnormal既定。restricted判定は`exists[bridge,vid] && mode[port,vid]==normal`であり、受信port自身の明示rowを必要としない。初期fixed source、restricted判定、RSTP固定VID maskはこのcatalogだけを見る。明示fixed rowはS上のprotocol static sourceを選択するもので、DIR Sへ自動追加しない。

Full Participantでは全port×4094の状態保持能力を確保する。`max_state_entries >= len(ports)*4094`をcheckedで検査する。状態は一keyにApplicant/Registrarを併置する。AにないVIDも規範状態を保持できる。`max_attributes_per_pdu`は1..4094、受信の重複もcountを加算する。`max_pdu_bytes`は46..1500の整数（paddingを含むMAC client bytesの最小長と通常MTU）、`max_sources_per_record/max_groups`はmembership専用でMVRPではfield自体を拒否する。その他capacityは統合設計の正整数。timer上限は共有媒体の全leave timerも保持できる場合`ports*(4094+3)`、p2p選択ではportのjoin/leaveall/periodicと実際の生存key数を制限する。capacity不足を黙ってstate省略へ変えない。

開始時：全key A=VO/R=MT、leave timer無し。local需要と登録由来需要を空にし、管理fixedをINへ、forbiddenをMTへ投影する。LeaveAll Begin、Periodic Begin後にperiodic_enabled=falseならDisabledを適用する。new_onlyはLeaveAll/Periodic機械を動作させない。non_participantはApplicant TXを抑止するがRegistrarは独立に動作する。初期local trueとfixed登録をsourceとしてMAPを評価し、必要なNew/Joinだけを配る。初期snapshotとtimer予約は一deltaでcommitする。

## 3. MACとMVRPDU codec

DA=`01:80:c2:00:00:21`、EtherType=`0x88f5`、untagged。SAは共通config rootの必須`port_identities:[{port,source_mac}]`を唯一の正本として参照する。source_macは既存の正準colon形式の非0 unicast MACで、port重複・MAC重複を拒否し、全active RSTP/MVRP port unionを過不足なく列挙する。adapter内source_macは受理しない。MAC byte offsetはDA0..5、SA6..11、EtherType12..13、MVRPDU14以後、最後4byteがEthernet CRC32のlittle endian FCS。FCS検査は共通wrapper。DA/Type不一致、tagged frameは対象revisionへdispatchしない。MVRP frameはbridgeが消費しdata relayに再投入しない。MAC operational=falseならTXしない、RXは機械へ渡さない（10.1 PDF275）。discarding portでもMAC operationalなら受信/送信する。

以下はMVRPDU先頭をoffset0とする。全複数byte整数はbig endian。

| offset | field | 幅と検査 |
| --- | --- | --- |
| 0 | ProtocolVersion | u8。TX0、RX0以上は実装版0の形式で読む |
| 1、以後Message先頭m | AttributeType | u8、VID型=1、0は終端判定専用 |
| m+1 | AttributeLength | u8、VID型は2 |
| m+2、以後Vector先頭v | VectorHeader | u16、`la=h>>13`, `n=h&8191`。la0/1。h0はAttributeList終端 |
| v+2 | FirstValue | AttributeLength byte、VID型はu16。n0でも存在 |
| v+4 | ThreePackedEvents | `ceil(n/3)`byte。FourPackedEvents無し |
| 次vector位置 | 次Vector又はEndMark | vector幅は`2+AttributeLength+ceil(n/3)` |
| List終了後 | 次Message又はEndMark | EndMarkは2byte0。AttributeListLength fieldは存在しない |

VID型のevent digitはNew0、JoinIn1、In2、JoinMt3、Mt4、Lv5。3eventを`36*a+6*b+c`でpackする。unpackは`a=b8//36,b=(b8//6)%6,c=b8%6`、有効byte範囲0..215。最終組の未使用digitはTX0、RXでは無視（非0であっても拒否しない）。count nは0..8191だがVID範囲とPDU容量により更に制約される。n0はla1の時のみvectorとして有効でFirstValueは検査せず読み飛ばす。`h=0000`を「不正zero count vector」と解釈せずEndMarkとして扱う。

n>0ならraw FirstValueを0..4094に限定し、checked `first+n-1<=4094`を要求する。raw0は受信portのPVIDへ置換する（11.2.3.1.7 PDF320）。内部VID0を新設しない。raw0から連続するvectorでは最初だけPVID、次はraw1、2…なのでPVIDとの重複をそのままwire順に処理する。TX zero置換を行う場合は置換後wire VID順で分割し、同一VIDを二重出力しない。

parserは境界cursorで次の順に動く：MAC/FCS/長さ→版→全Message/Vectorの構造→event配列→delta試算→capacity→commit。受信PDUの途中に違反があればprefixを含む全state、MAP、timer、TXをrollbackする。wire順で同じVIDが複数回現れること自体は合法。LeaveAllがあるvectorは、port上の全VIDへrLAを先に一度送り、その後n個のeventを順番に送る。別vectorのLeaveAllは別eventとして扱う。

版0で未知AttributeType/不正Length/予約LeaveAll/使用event不正は全PDU drop。未来版では未知Messageを、当該AttributeLengthと現行vector幅でEndMarkまで構造的にskipする。既知VID型の未知eventを含むvectorは全体をskipし、後続vectorを読む（10.8.3.5 PDF304）。予約LeaveAll、range overflow、途中切れ等の構造異常を未来版だから無条件受理しない。未知MessageでもLength0、境界外読取は禁止。未来版対応は新構文を推測するものではない。

物理PDU終端が**完全なVectorの直後**なら二重EndMarkを省略しても受理する。途中のFirstValue/packed byte、1byteだけのEndMarkは全drop。少なくとも一つのMessageとその中の完全なVectorを要し、空Message、版だけ、版+EndMarkだけは不正とする。明示PDU EndMark後のbyteはprotocolとして解釈せずMAC padding扱いで無視する。EtherType frameにはPDU lengthが無いため、MAC paddingを末尾0のheuristicで除去しない。フレーム受信ではFCS直前までをclient領域として読む。exact payload codec刺激はこの境界を既知lengthとして与える単体試験であり、短いpayloadをpaddingしたwire刺激と同一視しない。

canonical encoderは版0、型1、長さ2、必要eventをwire VID昇順の連続vectorへまとめ、未使用digit0、List EndMarkとPDU EndMarkを必ず出す。LeaveAllだけなら`00 01 02 20 00 00 00 00 00 00 00`（11byte）。VID1 Newだけなら`00 01 02 00 01 00 01 00 00 00 00 00`（12byte）。収まる必要eventを最大限詰め、途中のvectorもcountを調整して分割する。EndMarkの4byteを常に予約する。最小一eventを詰められないcapacityはprepare拒否（max_pdu_bytesの下限46はこの条件も満たす）。optional actionだけのPDUは送らず、optionalをgap埋めに使う最適化は本revisionでは行わない。

## 4. Applicantの全機能規則

状態集合は`VO,VP,VN,AN,AA,QA,LA,AO,QO,AP,QP,LO`。各規則の未記載sourceはstate維持、action無し。同じeventを理由なく再発行しない。以下はTable10-3 (PDF294)をeventごとの変更集合に再構成したもの。fixtureはsource全12状態と両p2p値、Registrar IN/MTを列挙する。

- A1 Begin：全状態をVOへ。New：VN/ANを維持し、それ以外をVNへ。
- A2 Join：VO/LO→VP、LA→AA、AO→AP、QO→QP。Lv：VP→VO、VN/AN/AA/QA→LA、AP→AO、QP→QO。
- A3 rNew：Applicantは不変。rJoinIn：AA→QA、AO→QO、AP→QP。p2p=falseのときだけ更にVO→AO、VP→AP。
- A4 rIn：p2p=trueのAAだけQA。rJoinMt/rMt：QA→AA、QO→AO、QP→AP、LO→VO。
- A5 rLv/rLA/Re-declare：VO/AO/QO→LO、AN→VN、AA/QA/AP/QP→VP。VP/VN/LA/LOは維持。pending Leaveを受信LeaveAllで消さない。
- A6 periodic：QA→AA、QP→AP。
- A7 tx：VP→AA(sJ)、VN→AN(sN)、AN→QA(sN。ただしRegistrar非INならAA)、AA/AP→QA(sJ)、LA→VO(sL)、LO→VO(s)。VO/AO/QO/QPは維持(optional_s)、QAは維持(optional_sJ)。
- A8 txLA：VO/AO/QO→LO(optional_s)、VP→AA(s)、VN→AN(sN)、AN→QA(sN、Registrar条件無し)、AA/AP/QP→QA(sJ)、QA維持(sJ)、LA→LO(optional_s)、LO維持(optional_s)。
- A9 txLAF：VO/LA/AO/QO→LO、VP/AA/QA/AP/QP→VP、VN/AN→VN、LO維持。actionは無い。

stateが変化してVN/AN/AA/LA/VP/AP/LOに入った時にTX要求を立てる。必須TXを詰められない通常txではstateを進めずTX要求を残す。txLAの容量不足はtxLAFを使い、stateが同じでも未送信需要のTX要求を保持して次の機会を予約する。optional actionの省略は記載したstate遷移を省略する理由にならない。`sN=New`、`sL=Lv`、`sJ=Registrar INならJoinIn/それ以外JoinMt`、`s=INならIn/それ以外Mt`。wire送信はTX transactionで確定したRegistrar snapshotを読む。

normalは全eventを処理する。new_onlyはBegin/New/tx以外を無視し、到達状態VO/VN/AN/QA、sNだけを送る。new_onlyのAN txはNew-only選択によりQAへ移る（full participantの非IN→AA分岐で未許可stateを作らない）。non_participantでは外部宣言需要を保持しても送信actionを作らず、復帰時Beginと現在の需要のNewを適用する。Full→New-onlyなどのmode変更はold timer世代をcancelしてBeginから再初期化し、旧TXを引き継がない。

## 5. Registrarと管理override

Registrar状態INは登録、LVもLeave indicationまで登録維持、MTは未登録。以下以外のeventはno-op（rIn/rMt/Applicant local eventは登録を作らない）。Table10-4、10.7.2、PDF287–288/295。

- R1 Begin：MT、旧leave timer取消し。再初期化時の既存登録撤回はapplication reset transactionで一度だけ通知する。
- R2 rNew：どの状態からもIN。LVならtimer取消し。毎回new=trueのJoin indication。既存INでもnew通知を省略しない。
- R3 rJoinIn/rJoinMt：MTからINにしてJoin indication。LVからINはtimer取消し、重複Join indication無し。INは維持。
- R4 rLv/rLA/txLA/Re-declare：INからLVへ移りleave timer開始。既にLVなら延長しない、MTなら何もしない。p2p=trueではLV待ちを省略し、直ちにLv indicationとMT（表の注a）とする。
- R5 Flush：IN/LVならLv indication、その後MT、leave timer取消し。MTもMT。
- R6 有効なleavetimer満了：LVだけLv indicationを出してMT。IN/MTには作用無し。
- R7 fixed_ignore_new：常にIN、受信eventは無作用。fixed_propagate_new：常にIN、rNewだけNew indication。forbidden：常にMT、受信無作用。timerは不要。管理変更時は旧/新登録差のJoin/Lvを一度発行し、fixedへは静的sourceを付ける。

Join/New indicationに対しapplicationは当該port/VIDのDynamic VLAN Registration Entryを作成/更新し、成功したindicationだけをMAPへ渡す。restricted=trueならStatic VLAN Registration Entryが存在し、そのport/VIDの管理値がnormalでなければdynamic登録を抑止する（11.2.3.2.2/3 PDF321）。この失敗は不正wire扱いではない。Registrarの規範状態とapplicationの登録成功状態は別fieldで、registration_failed記録を出す。容量はprepareで全VID分を確保して途中のFDB容量失敗を避ける。New-onlyのnew伝播には対応するfixed_propagate_new static entryを要する（11.2.6 PDF322）。

`W`はapplicationで登録成功したwire集合、fixedのstatic sourceは正規化SVRE catalog側で保持する。S由来fixedと管理row由来fixedを区別してprovenanceを残し、wire Wへ混ぜない。通常のR=IN又はLVで成功登録がある間Wを保持し、Lv indicationでWを除く。A外VIDも通常はWに入り得る。data許可だけを`S ∪ (W ∩ A)`とする。W更新がA外という理由でPDUをdropしない。Sのtagged値優先、W側はAのtagged値。local declaration単独はWを立てない。wireのLvでSを削除しない。

## 6. LeaveAll、Periodic、timer、TX

LeaveAllはport単位Active/Passive。BeginとrLAはどちらからもPassiveにしてleaveall timer開始。leavealltimerはどちらからもActive、timer再開始、TX要求。ActiveでtxならsLAを一度作りPassive、Passiveでtxは無作用（Table10-5 PDF295）。sLAは同一port全Applicant/RegistrarへのrLAも発生させる（10.7.5.20(a)、10.7.6.6、10.7.9）。このrLAと送信機会txLA/txLAFは別eventとして両方を保持する。自己生成rLAの宛先はApplicant/RegistrarでありLeaveAll自身へ拡張しない。外部受信rLAはLeaveAllへ一度渡す。timer再開始はBegin、受信rLA、leavealltimer時だけで、Activeのtx/sLA自体では再開始も乱数消費も行わない。

Periodicはport単位Active/Passive。BeginはActive+timer開始。Activeのperiodictimerは再開始+全Applicantへperiodic、Passive満了は無作用。EnabledはPassive→Active+timer開始、既にActiveならno-op。DisabledはActive→Passive、既にPassiveならno-op。Disabledでは規範表どおり既存timerの満了を無作用として消費し、新しいtimerをarmしない（Table10-6 PDF296）。

設定timer上限はcs→psとLeaveAll上限3L/2がu64内に収まる値を選んだDIR制約。実deadlineのnow+durationも別にcheckedとする。時間はps整数、1cs=`10^10`ps。timer key slotは`join/leave/leaveall/periodic`、leaveだけVIDを持つ。checked加算、token世代、stale無作用は共通契約。LeaveTimeはそのままperiod、Periodicは`10^12`ps。LeaveAllTime=0ではarmせず、0→非0管理変更はBegin。0へ変更した際は旧世代cancel。LeaveAllのsampleは厳密な開区間`L<T<3L/2`内の整数ps。標準推奨の`LeaveTime >= 2*JoinTime + 6*timer_resolution`をprepare診断に出す（本revision resolution=1ps、満たさない設定を自動補正しない）。LeaveAllをLeaveTimeより十分長くする勧告もmetadataに残す。

再現乱数はDIR固有：SHA-256入力をUTF-8 `mvrp-la-v1\n<seed>\n<bridge>\n<port>\n<counter>\n`とし、先頭8byteをbig-endian u64とする。counter初期0、試行ごとchecked+1。候補数N=`ceil(L/2)-1`、`x < 2^64-(2^64 mod N)`になるまで再標本化し、T=`L+1+x mod N`。seed等はcanonical設定値。timer再開始ごとにsample一つを消費、RX rollback時にはcounterも戻す。標準は乱数algorithmを指定しないためこの式を規範の値として引用しない。

p2p TX pacerは要求の最初をnow+1ps、以降`max(now+1,last_opportunity+ceil(JoinTime/2)+1ps)`にする。これはDIRの送信機会提供制約であり、1.5JoinTimeの任意の閉区間内を3回以下にし、通常の次要求をJoinTime内に満たす。未消費要求は一つにcoalesceしjointimerで機会を作る。timer0の再armをしない。共有媒体機械単体modelでは未予約要求を[0,JoinTime]から標本化し、0の場合もFESではnow+1psへquantizeする。実topologyのshared LANはprepare対象外。

TX処理順は以下のDIR決定順で固定する（局所state copyに対する一transaction）。条項が機械間の全実行順序を一意に指定しているという主張ではない。規範のrLAとtxLA/txLAFを省略せず、送信機会を先に確定してから次の順で適用する。

1. 送信機会時にLeaveAll Activeかをsnapshotし、今回LeaveAllを送ることとそのvector領域を確定する。
2. LeaveAll機械にtxを与える。ActiveならsLAを出してPassiveへ進む。ここではleaveall timerを再開始しない。
3. sLA由来のrLAを全local Applicant/Registrarへ各一度配送する。Registrar INは共有媒体ならLV+leave timer開始、p2pならLv indication+MTへ進む。LeaveAll自身には配送しない。
4. port/VID昇順でRegistrarへtxLAを先に与え、続いてApplicantへtxLA、容量不足ならtxLAFを与える。rLA後のRegistrar LV/MTに対するtxLAは無作用なのでleave timer/indicationは二重発生しない。通常のLeaveAll無し機会はApplicantへtx、必須actionが入らなければstateを戻しTX要求を残す。送信actionはこの時点のRegistrar状態を読む。
5. LeaveAllをその応答属性eventより先にencodeする（10.8 NOTE 2、PDF298）。通常txでoptionalしかない場合はPDU無し。LeaveAllはcount0でも送る。必要容量、FES枠、TX枠をpreflightする。
6. 中間遷移で生じた送信要求も保持し、state・timer・PDU・recordを一括commitする。例えばAA/INのp2pはrLAでVP/MT、その後txLAでAA/Mt送信となり、AAへのentryが作る後続TX要求を消さない。packetが媒体で失われてもstateを巻き戻さない。後続TX/Periodic/LeaveAllが回復を担う。


## 7. MAP、new、tree変更と安定化

MAP Context0の集合Fは**当該adapterが所有するbridge/context内**で`link_up && forwarding`を満たすport。別bridgeのportを集合へ混ぜない。state Learningは含まない。MVRP RX/TX可否はFではなくMAC operationalで決める。各VIDについて外部sourceを`local(port)`、`registered(port)`、static fixed sourceとして保存し、MAPが生成したApplicant需要をsourceへ戻さない。port Pの宣言需要はP自身のlocal需要と、F内のP以外のsourceの和集合で決まる。localのrequestもF内なら他portへ伝播する。単なるOR値の反復だけではnew通知を失うので、newは一回性eventとして別に保持する（10.2/10.3 PDF276–278）。

Join indication/requestがF内のQから来たらP≠Qの全FへJoinを配る。`new || tcDetected[Q]>0`ならNewとする。Leaveはsourceを先に除き、P以外にsourceが無いPだけへLv。複数sourceの一つの撤回で他source由来の需要を消さない。F追加は追加portのsourceを他へ伝播し、他portのsourceを追加portへ伝播する。F削除は残留各Pについてsourceの有無を再評価し、不要となった需要だけを撤回する。削除port自身の宣言はLvで撤回する。local需要の保存と実際のApplicant宣言を区別し、F復帰までは保存local需要の自動再投入を抑止する。MAC downならLeaveは送れないのでpeer側のprotocol agingに任せる。local需要は管理で消されるまで保存し、リンク復帰で再宣言する。

RSTPの一policy commitから旧/新role、旧/新forwarding、tcDetectedを受け取る。同じtree deltaの順序は、(1) Root/Alternate→DesignatedへRegistrar FlushとLeaveAll leavealltimer、(2) Designated→Root/AlternateへApplicant/Registrar Re-declare、(3) Fの削除/追加、(4) MAP需要再評価。この順序で旧登録が新treeへ再伝播する情報loopを防ぐ（10.3.1.1、10.7.5.2/3、PDF278–279/289）。無関係なrole変更で全VID Flushを捏造しない。MAC downはF削除とtimer/transport世代切替を行い、再有効化はBegin+現在local需要と管理sourceから再構築するDIR lifecycleとする。

NewがMAD Join indication又はrequestとして届いたport/VIDでは、そのport/VIDに対応するDynamic Filtering Entryだけを削除する。static FDB/VLANや別VIDは保持する。受信source portだけでなくMAPの宛先portでもnew処理をする（11.2.5 PDF322）。このflushはWの登録/撤回とは別effect。既存DIRのdynamic MAC学習表への具体的な削除・aging・learning gateは[統合設計5.6](RSTP・MVRP-wire実装設計書.md)を正とする。固定VLAN登録の管理状態と静的MAC FDB entryを混同しない。

局所worklistは`(cause_id,port,vid,primitive,new)`で同一配送を一回に限定する。MAP requestから新しいMAP source/indicationを合成しないためローカル循環を作らない。受信Newごとに別causeなので正当な後続Newを消さない。state entry/request/indication/timer操作を各1stepとしてmax_control_stepsを計数し、上限超過は全delta破棄のrun failure。正常callbackは有限VID×portのworklistを消費して停止する。future TXは最低1ps後、wire RXは媒体serialization/propagation後で、現在callbackへ自己RXを同期注入しない。

内部生成PDUは媒体のpeer ingressからprotocol/bridge所有adapterへ配送し、送信元adapter_idに受信先を固定しない。外部packet.adapter_idは境界刺激の対象指定であり、複数bridgeの能動交換を妨げない。

同時刻は共通契約どおりphase0 timer、phase1 RX、phase2 TX。leave満了とJoin受信が同時ならまずLv/MT、その後Join/IN。同一phaseは共通stable event sequenceを使う。MAP内部はbridge/port ID、VID、primitiveの安定順。W変更の世代を更新し、queue再判定とpolicy snapshotを同deltaへ入れる。active data copyは中断しない。

## 8. 実装単位と独立期待値

`decode(frame)`は全構造を検査して`DecodedMvrp{version,vectors,events}`を返し、副作用を持たない。`step(snapshot,event)`はApplicant/Registrar/LeaveAll/Periodic、application、MAPの局所copyから`ProtocolDelta`を作る。`encode(tx_intents)`はcapacity境界を含むcanonical byteを作る。effectはwire登録、local FDB flush、timer arm/cancel、TX enqueue、state trace。state traceはport/VID、event、前後Applicant/Registrar、管理mode、action、cause、timer key/token、MAP source差を記録する。W差だけでは機械を再生できないため全state遷移を出力対象とする。pure reducerとwire codecを製品コードへ組み込む際は本書のrulesを通常のRust enum/matchへ写し、fixture generatorを製品実装へ転用しない。

[独立vectors](../fixtures/mvrp-standard-v1.json)は`schema_version=1`、`status=design_vectors_not_product_tests`。各要素は`id/test_id/kind/input/expected/basis`、test_idは既存P-MVRP-T01。V101以降を使う。`fsm_*`は機械の一event刺激であり正常PDU入力ではない。`wire_mac_frame_fcs`だけがDAからFCSまでの実wire刺激。`codec_payload_exact`は長さ付きcodec単体、`packing_arithmetic`は算術、`integration_rule`はMAP/FES/投影の独立期待である。

coverageはA1–A9の全12source×16event×p2p2値×Registrar2値、R1–R6の全3source×12event×p2p2値、LeaveAllとPeriodicの全2state×4eventを含む。表の維持/no-opも省略しない。追加fixtureは管理override、New-only到達制約、容量不足、LeaveAll local配送、MAP追加/削除、new flushを対象とする。sJ/sのRegistrar LV分岐はMTと同一という規則を明示し、LVをINと誤る回帰を個別刺激する。

設計vectorのJSON構文、ID一意、base6再計算、MAC CRCの独立再計算、参照リンクとdiffを検査する。これは製品FSM、Rust実装、pcap相互運用、実時間performance、標準適合認証の合格を意味しない。製品実装後の受入れでは全matrix、管理mode、MTU分割、三bridgeの宣言/撤回・topology変更・同時刻競合を同じpure reducerと実wire経路の両方で実行する。

2026-10-17採用作業記録：本書のP-IDを[正式採用仕様](../../specs/後続拡張採用詳細機能仕様書.md)へ接続した。[現行API・scope](../../guide/reference/後続10分野の現行実装と利用境界.md)と[製品検証記録](../../verification/results/extension-product-2026-10-17.md)を参照。設計vectorと過去のdesign-only報告は履歴のまま保持し、製品合格とは区別する。文書版・履歴は本pushの改訂として確定した。
