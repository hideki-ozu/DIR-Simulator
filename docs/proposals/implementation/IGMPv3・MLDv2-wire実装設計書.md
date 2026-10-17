# IGMPv3・MLDv2-wire実装設計書

文書バージョン：`1.1.0`
<br>
対象GitHubバージョン：`v1.1.6`
<br>
文書ID：`proposal-membership-wire-implementation`
<br>
文書状態：承認済み限定scopeの現行実装済み（v1.1.6収録）。本書は採用した詳細契約。製品試験の実施範囲・最終判定は別記録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-17 | 受動snooping role、wire codec、保守的source-filter lease、原子的epoch更新と独立fixtureを初版化。v1.1.6収録版を確定 |

## 1. 要件・採用範囲

<a id="p-igmp-r01"></a>
**P-IGMP-R01**：IPv4 IGMPv3 Report/Queryの実packetを検証し、VID・portごとの源別membershipとrouter-portを学習・失効させ、Ethernet転送候補に反映する。packet parsingだけを完成条件にしない。

<a id="p-mld-r01"></a>
**P-MLD-R01**：IPv6 MLDv2について同じ能力を提供し、IPv6 envelope、Router Alertとpseudo-header checksumを必ず検証する。IPv4との状態混同を許さない。

初期roleは両familyとも **passive_conservative_snooper_v1**。bridgeはReportを抑制・代理生成せず、Queryを生成せず、querier選出へ参加しない。外部querier/hostは入力packet workloadとして明示する。RFCのrouter状態表を部分実装してrouterと称する方式ではない。保守的leaseの算法は本提案固有でありRFCが規定したsnooping状態機械とは称さない。これによりP-WIREのmembership/snooping目的を維持し、生成・再送・選出は「このroleは送信しない」と確定する。適合表示は `IGMPv3/MLDv2 wire decoder + conservative snooping` のみ。

対象は単一C-VLAN、既存全二重Ethernet/VLAN動的policyとの接続。IGMPv1/v2・MLDv1はvalid旧版の透過のみでmembership学習しない。旧版検出時は当該bridge/VID/familyをrun終了まで `compatibility_flood` にし、データを全eligible portへ転送する。復帰timerはない。旧版packetから源別membershipを捏造しない。RFC3376/3810の現行版と同じwireは受理するが、設定revision別名は許さない。host/listener、multicast router、proxy、explicit tracking、fast leave、query timer shortening、retransmission、IP fragmentation/reassembly、QinQ、IP routing、RA extension semanticsは対象外。旧版混在時の帯域削減を保証しない。

## 2. 機能と固定一次資料

<a id="p-igmp-f01"></a>
**P-IGMP-F01**：IPv4 envelope→IGMP decode→lease union→membership projection→既存egress選択までを提供する。

<a id="p-mld-f01"></a>
**P-MLD-F01**：IPv6 envelope→ICMPv6 decode→同一のlease union→独立IPv6 membership projectionを提供する。

照合日：2026-10-17。採用版は[ RFC 9776 (March 2025)](https://www.rfc-editor.org/rfc/rfc9776.html) §4、§7、§8と[ RFC 9777 (March 2025)](https://www.rfc-editor.org/rfc/rfc9777.html) §5、§8、§9。両者はそれぞれ3376/3810を置換する。[9776 errata](https://www.rfc-editor.org/errata/rfc9776)、[9777 errata](https://www.rfc-editor.org/errata/rfc9777)は照合時 `No matching errata found`、適用errata集合は空。将来取得時の追加errataは自動適用しない。

Snooping参考は[ RFC 4541](https://www.rfc-editor.org/rfc/rfc4541.html) §2.1.1、§2.1.2、§3（InformationalでありIGMPv3/MLDv2完全状態表ではない）。IP envelopeは[ RFC 791 §3.1](https://www.rfc-editor.org/rfc/rfc791.html)、[RFC 8200 §3–4](https://www.rfc-editor.org/rfc/rfc8200.html)、Router Alertは[ RFC 2113 §2](https://www.rfc-editor.org/rfc/rfc2113.html)、[RFC 2711 §2](https://www.rfc-editor.org/rfc/rfc2711.html)、checksumは[ RFC 1071 §1](https://www.rfc-editor.org/rfc/rfc1071.html)。以下でRFC由来wire値と本提案固有のrole判断を区別する。

## 3. 詳細仕様

<a id="p-igmp-s01"></a>
**P-IGMP-S01**：本節の共通設定、IPv4 layout、IGMP recordと転送規則を所有する。

<a id="p-mld-s01"></a>
**P-MLD-S01**：本節の共通設定、IPv6 layout、MLD recordと転送規則を所有する。

### 3.1 設定・入力

候補profileは `ethernet.control.wire.experimental.v1`。現行dynamic profileへの暗黙オプション追加はしない。全JSON objectは未知field拒否、欠落必須field拒否、null拒否。整数に浮動小数/負数/文字列を許さない（psだけはu64正規十進文字列）。省略defaultは下表だけ。未知enumはprepare失敗。

| field | 型・範囲・単位 | 必須/default・0 |
| --- | --- | --- |
| id / protocol | 共通wrapperで定義。protocol=igmp又はmld | 必須、idはrun内一意 |
| protocol_revision | `igmp-rfc9776-2025` / `mld-rfc9777-2025` | 必須、errata集合空を固定 |
| role | `passive_conservative_snooper_v1` | 必須 |
| ports | 既存switch ingress port IDの非空・重複なしarray | 必須、同一family/port重複拒否 |
| max_pdu_bytes | u32、64..1500 byte（IP packet全体、既存serializerのpayload上限を継承） | 必須、0拒否 |
| max_sources_per_record | u32、1..4096 | 必須、0拒否。wireのN=0は有効 |
| max_groups | u32、1..65536、実在bridge/VID/family/group/port key数 | 必須、0拒否 |
| max_filter_leases | u32、1..262144、全snooper内のdistinct filter key数 | default 4096、0拒否 |
| max_sources_total | u32、1..1048576、各filter lease source数の総和 | default 65536、0拒否 |
| membership_interval_ps | u64 ps、1..u64::MAX | default `260000000000000` (2×125s+10s)、0拒否 |
| router_interval_ps | u64 ps、1..u64::MAX | default `260000000000000`、0拒否 |
| packets | 共通workload内のrecord array、空可 | 必須（adapter内には置かない） |

設定rootとpacket tagged unionは[共通仕様](../後続拡張契約提案仕様書.md#wire)を正とする。本表は各membership adapter内のfieldである。packet recordは `id`（非空UTF-8、run内一意）、`adapter_id`、`ingress_port`、`arrival_ps`、`encapsulation`（固定`ip_packet`）、`bytes_hex`、`vid`（1..4094）、`source_mac`（正規unicast 48bit MAC）、`destination_mac`（正規multicast 48bit MAC、既存serializerの予約L2制御MACは不可）を必須とする。hexは空不可、偶数桁、ASCII 0–9a–fA–Fのみ。bytesはIP header開始、Ethernet/FCS/paddingを含まない。packetのrevisionはadapter_idから一意に取得し、packetへのprotocol_revision重複指定を拒否する。ingressに対応するoutput policyの静的member又はdynamic.registrableがそのVIDを許可することをprepare検査する（既存member_port規則）。静的memberとregistrableの同一VID重複は既存validatorで拒否する。未登録時刻のpacketは動的eligibility判定でfilterする。

`arrival_ps`は完全なpacketのingress受信完了時刻。source linkの受信wire積算を再現するため、serializerが求めるpreambleからEOFまでの時間Dに対しarrival>=Dを必須とし、[arrival-D,arrival)を境界入力のRX wire実績とする。IFGはarrivalから別に予約し、次packetの開始との重なりをprepare拒否する。現在のserializerはC-tagでもpayloadを46byteまでpadするので、MAC bytes=18+max(46,bytes_hex decoded length)+4、D=ceil((MAC bytes+8)×8×10^12/bitrate)、release=(arrival-D)+ceil((MAC bytes+20)×8×10^12/bitrate)。checked演算する。同一linkの通常source workloadと境界入力の併用を拒否する。以後のbridge転送copyは通常queue/wireで実送信する。この境界入力は送信元host実装の証明ではない。frameの両MACは明示指定値だけを使い、prepare時にL3から導出しない。L3検証に成功したpacketだけ、destination_macとIPv4 multicast下位23bit又はIPv6下位32bitのmappingを比較し、不一致はruntime control_drop(reason=destination_mac)にする。切詰め・不正L3でも指定MACと実bytes長からframe/FCS/時間を確定でき、runtime-dropをprepare拒否へ変えない。source_macは指定値、tagはvid、PCP=0、DEI=0から既存serializerで作る。L3 packet長をEthernet payload長として一度だけ計上する。L2最小padding・FCS・preamble・IFGは既存serializer所有。

全packet（T以後含む）のhex/revision/静的長上限をprepare検査する。checksum/semantic不良はruntime control_dropの対象としてprepareを通す。protocol_revisionとIP versionが不一致ならruntime envelope drop。arrival+L/Rは実効lease生成時にchecked加算し、overflowはcallback失敗。adapter enabled protocol/portでは同じmembership/router keyへabstract操作を設定したrunをprepare拒否。static group/router設定は許可し、unionによる影響を結果へ示す。初期dynamic lease/routerは空、compatibility_flood=false。

### 3.1.1 境界wireイベント・停止prefix

境界packetはL3の正当性と独立した物理入力であり、prepareでpacketごとにBoundaryStart(S)、BoundaryArrival(A)、BoundaryRelease(R)の3件を予約する。S=A−D、A=arrival_ps、Rは上式、S<=A<R。Start/Releaseはphase0、Arrivalはphase1。同時刻phase0は既存wire完了・link/registration→wire expiry→BoundaryRelease→BoundaryStartの順とする。同種packetはidのUTF-8 byte順、Arrivalは通常FES seq順。AでRX完了stateとmembership処理を一つのNetworkDeltaへ含める。phase1から同時刻phase0を新規予約しない。

全3予約・audit容量・ID・時間をprepareでchecked検査し、共通pending event上限を超えればprepare失敗。T以後の予約も計数し、実行対象だけが[0,T)。start前のwire statusはplanned、Startでreceiving、Arrivalでreceived、Releaseでreleasedとなる。boundary slotはingress方向ごと容量1、queueなし。全packetの[S,R)をprepareで重複検査し、重なりはBoundaryOverlap拒否、前packetのR=次Sは許可。boundaryは外部からの物理入力なのでStartでlink/VIDの状態を理由に占有を取り消さず、Arrivalのingress eligibilityで受理/破棄する。途中のlink downも受信時間・既に始まったframeを巻き戻さない。copyの通常queue容量不足は既存規則の正常capacity drop、境界slot重複をそのdropへ流さない。

各eventのaudit/state/metricsは当該callbackの原子的deltaとする。Arrivalでdecode不良でもRX完了・control_dropはcommitし、membership変更/copy生成は0。資源不足ならArrival callback全体が失敗し、receivedへの移行も公開しない。Start/Releaseはpolicy_epochを変更しない。Startが未実行なら受信実績0、Arrival未実行ならmembership/control_drop実績0、Release未実行ならactual_release_ps=null。Tちょうどのeventは未実行である。

境界の予約metadataはpacket_id/ingress_port/source_mac/destination_mac/vid/bytes_hex/bitrate_bps/mac_bytes/planned_start_ps/planned_arrival_ps/planned_release_psを持つ。metadataのbitrate_bps/mac_bytes/予定時刻はu64十進文字列、vidはu16 JSON整数、ID/MAC/hexは上記string型。予定metadataはprepare済み入力の全packetを列挙する。実績record schema `ethernet.wire.boundary/1` は必須field `effect_seq,packet_id,ingress_port,time_ps,event,status,actual_start_ps,actual_arrival_ps,actual_release_ps`。event=start/arrival/release、status=receiving/received/released、未到達actualはnull、他時刻/seqはu64十進文字列。成功eventごと1行、予定を実績fieldへ複写しない。snapshotではmetadataと最後の成功行からplanned/receiving/received/releasedを再構築する。

観測終端HにおけるRX wire時間はStart成功時だけmax(0,min(H,A)−S)、IFG時間はArrival成功時だけmax(0,min(H,R)−A)、それ以外0。積算単位はps、RX完了frame/byte counterはArrival成功時に一回だけ+1/+mac_bytes。bit数は全frame完了時だけ(mac_bytes+8)*8を加え、途中frameの端数bitを整数へ丸めて補完しない。物理占有時間と完了byteは別指標。RX入力を通常forwarded TXへ二重計上しない。IFG時間はRX wire時間へ含めず独立に出す。metricsは成功prefixのfrozen viewで算出し、exportの繰返しでlive stateを変更しない。

### 3.2 IPv4とIGMP byte contract

すべてoffsetはbyte、multibyteはnetwork byte order。IPv4 offset0上位4bit=4、下位4bit×4=H（20..60）、2:u16 total length=bytes長、6:u16 fragment offset=0かつMF=0、8:u8 TTL=1、9:u8 protocol=2、10:u16 header checksum、12:4 source、16:4 destination。header全体をchecksum検査。optionsは20..HをEOL=0、NOP=1、他はtype/len>=2/TLV境界で走査、Router Alert `94 04 00 00`がちょうど一つ必要。未知optionは長さ確認後ignore。IPv4 reserved flagは0、DFは任意。fragmentはunsupported_fragment drop。sourceはunicast又はReportだけ0.0.0.0、Queryの0.0.0.0はdrop。sourceのloopback/multicast/limited-broadcastはdrop（本profileの入力制約）。

IGMP先頭p=H。共通0:type、1:code/reserved、2:u16 checksum。Query type=0x11：4:4 group、8:u8 flags/S/QRV、9:u8 QQIC、10:u16 N、12:4N sources。minimum=12+4N。上位4bit flagsは記録してignore、S=bit3、QRV=bits0..2。Report type=0x22：1:reserved、4:u16 flags、6:u16 M、8以降records。recordは0:type、1:aux_words、2:u16 N、4:4 group、8:4N sources、続く4×aux_wordsをskip。次recordまでの長さは8+4N+4aux_words。

groupは224.0.0.0/4、source-listはunicast（0.0.0.0、multicast、255.255.255.255拒否）、重複sourceは集合化するが上限はwire Nで検査。Report宛先224.0.0.22。Queryはgeneral(group=0,N=0)宛先224.0.0.1、それ以外group宛先、group=0,N>0拒否。checksumはIGMP全IP payload、追加末尾も含む。reserved/flags/aux/追加末尾は値を学習に使わず、許可される長さを検証したうえでignoreする。RFC9776 §4.1.10、§4.2.11–12の追加dataを「長さ不一致」と誤判定しない。

### 3.3 IPv6とMLD byte contract

IPv6 offset0上位4bit=6、4:u16 payload length=bytes長-40（0/jumbogram拒否）、6:u8 next header=0（HBH必須）、7:u8 hop limit=1、8:16 source、24:16 destination。最初のHBH offset40：0:u8 next header=58、1:u8 ext length、全長E=8×(extlen+1)。初期roleはHBH→ICMPv6のみ、他extension chain/fragmentはunsupported_extension drop（標準で有効な全IPv6 packetを扱うとは主張しない）。HBH offset2以降をPad1=0、他TLVは2+lengthで境界検査、PadN=1をskip、Router Alert type5/len2/value0000を一つ要求。未知optionは上位2bit00のみskip、他はdrop。IPv6 Query sourceはfe80::/10、Report sourceはfe80::/10又は::。Query/Report共にHL=1とRA必須。

MLD先頭p=40+E。Query type130：0:type、1:code=0、2:u16 checksum、4:u16 MRC、6:u16 reserved、8:16 group、24:u8 flags/S/QRV、25:u8 QQIC、26:u16 N、28:16N sources。minimum=28+16N。Report type143：0:type、1:reserved、2:u16 checksum、4:u16 flags、6:u16 M、8以降records。recordは0:type、1:aux_words、2:u16 N、4:16 group、20:16N sources、4aux_wordsをskip。record長20+16N+4aux_words。Reportのbyte1はreservedとしてignoreする（Query codeとの混同禁止）。

groupはff00::/8、list sourceはunicastかつ::以外。Report宛先ff02::16。general Query group=::,N=0の宛先ff02::1、specificはgroup宛先、group=::,N>0拒否。checksum対象はpseudo-header `src16 || dst16 || icmp_len:u32 || 000000 || 3a` とMLD全messageで、HBHを含めない。aux/追加data/reserved/flagsはIGMP同様に検証後ignore（RFC9777 §5.1、§5.2）。

共通checksum算法：checksum fieldを0として16bit big-endian語を加算、奇数末尾に計算上だけ00を足す、carryを下位16bitへ繰返し加算、最後にbitwise NOT。受信ではchecksumを含めてfoldした和=0xffffが必要。IP headerとupper checksumは独立して検証する。

### 3.4 control分類・転送

最初にcurrent ingress link/VID/tree eligibility、不適格はcontrol_drop(reason=ingress_ineligible)。次にenvelope長→envelope制約/checksum→message長→upper checksum→record構文/アドレス/上限の順で最初の違反だけを診断。未知record typeはP-WIRE共通契約に従いwhole-PDU drop（標準routerが未知recordを無視する場合との差を明示するstrict adapter policy）。既知6typeは次節。M=0/N=0は有効。packet内全record検証後にのみ状態を更新する。IP total/payload lengthの過不足はip_length、IP header checksumはip_checksum、upper checksumはchecksum、message/recordの切詰めはmessage_length、record typeはrecord_type、IP/group/source/destination値はaddress、wire source count超過はsource_limit、それ以外のheader/RA違反はenvelopeへ分類する。

valid現行Queryは全eligible port（ingress除く）へ透過、Reportはrouter port（ingress除く）へ透過し、router集合空なら全eligible portへ透過する。membership source filterで制御packetを抑制しない。未知IGMP message type/MLD関連未対応typeはunknown_message drop。IGMP旧Queryはmessage長8、旧Report type0x12/0x16又はleave0x17は長8、MLDv1 Query長24、Report131/Done132長24で分類する。旧版はMLD Report131/Done132のcode=0、IGMP old report/leaveのbyte1=0を確認する。IGMPv1 Report0x12もRouter Alert省略を許可する。旧版はenvelope・checksum・型固有group/addressを検査しvalidなら全eligible portへ透過＋compatibility_flood=true。旧QueryのRouter Alert省略IGMPv1も許容する（現行Query判定前に8byte版分類、IPv4他検査は維持）。旧Report/leaveはIPv4 multicast group、MLD multicast groupを要し、general旧Queryのみzero group可。旧Report/leave宛先はIGMP group/224.0.0.2、MLD group/ff02::2、旧Queryはgeneral all-nodes又はgroup。

unknown data multicastは全eligible portへflood。known groupはstatic+dynamic membershipのsource predicate OR router ports。IPv4 224.0.0.0/24及びIPv6 ff02::/16は常にflood（本roleの保守的例外）。compatibility_floodなら全data multicastをfloodする。これらの最終候補をlink/VID/tree eligibilityとingress除外で絞る。MAC multicast aliasではgroupを混同しない。通常dataのsource/groupは既存ip_multicast metadataを使い、payloadは不透明のまま保持する。null metadataは既存L2転送へ渡す。制御packetのIP値だけをwireから解析し、制御packetの宛先MAC対応も検査する。通常dataへIP parser／checksum／fragment検査を追加しない。

## 4. Architecture責務

<a id="p-igmp-a01"></a>
**P-IGMP-A01**：IPv4 decoderはbyte検証のみ、snooper plannerはIPv4 partitionの差分を返し、coordinatorが既存NetworkDeltaへ統合する。

<a id="p-mld-a01"></a>
**P-MLD-A01**：IPv6 decoderはpseudo-header検証まで所有し、同一plannerへfamily付きtyped messageを渡す。IPv6 partitionのkey/epoch所有はcoordinatorに限定する。

現在の接続先は `input/ethernet/dynamic.rs` のprepare、`lib/types/ethernet/dynamic.rs` のMembershipKey/RouterKey/SourceFilter、`runtime/ethernet/dynamic.rs` のDynamicState/plan_controls/apply、`runtime/network.rs` のNetworkDelta、`runtime/registered.rs` のEffectBatch予約、`lib/snapshot/ethernet/dynamic.rs` のDynamicControlRecordである。追加予定配置は `input/ethernet/wire.rs`、`runtime/ethernet/wire/{igmp,mld,membership}.rs`、`lib/snapshot/ethernet/wire.rs`。現在存在する実装と追加予定を混同しない。

`ProtocolAdapter::decode(packet,envelope)->Result<ValidatedMembershipMessage,ControlDrop>`は純粋関数。`plan_membership(now,message,&WireState,&DynamicState)->Result<WireDelta>`がmap/lease/projected filter/control records/forwarding candidatesを完成し、NetworkDeltaの一部として資源予約・commitする。adapterからFES/Registry/queueを直接変更しない。既存SourceFilterは単一期限しか持たないので、個々filter leaseをそのまま既存MembershipSet列へ変換してはいけない。WireStateが全leaseを所有し、集約filterだけをdynamic policy overlayへ投影する。overlayの期限はwire coordinatorだけが所有し、dynamic既存expiryとの二重取消しを禁止する。

## 5. 詳細設計

<a id="p-igmp-d01"></a>
**P-IGMP-D01**：本節のstate machineをfamily=ipv4に適用し、wire上限/lease/epochを同じcommitへ含める。

<a id="p-mld-d01"></a>
**P-MLD-D01**：同じstate machineをfamily=ipv6に適用し、IPv4と独立したlease/router/compatibility stateを持つ。

### 5.1 状態と全遷移

K=(switch,vid,family,group,ingress_port)。filter lease key=(K,mode,sorted unique source set)。value=(expires_ps,generation)。空INCLUDE leaseは作らない。port配下に複数hostがいてもreporterのIPを識別子にしない。過去の正の受信希望を期限までORで保持するため、他hostのleaveで誤削除しない。router lease key=(switch,vid,family,port)、value=(expires_ps,generation)。compatibility_flood key=(switch,vid,family)、value=bool、初期false。bridgeごとのwake_generation:u64 ledgerとnext_wake（時刻/token又はnull）もWireStateが所有する。

L=membership_interval_ps、R=router_interval_ps。refreshはexpiry=max(old_expiry,now+L)、routerもmax(old,now+R)。同じexpiryならgeneration/epochは変えない。違うfilter集合は別leaseを作る。異なるrecordをlatest-only置換しない。expiry<=nowのleaseはphase0で除去済みとする。

| 入力 | guard | action（wire record順にstaged stateへ適用） |
| --- | --- | --- |
| MODE_IS_INCLUDE (1) | S非空 | INCLUDE(S) lease insert/refresh |
| MODE_IS_INCLUDE (1) | S空 | no-op、旧leaseを削除しない |
| MODE_IS_EXCLUDE (2) | 全S | EXCLUDE(S) lease insert/refresh |
| CHANGE_TO_INCLUDE_MODE (3) | S非空 | INCLUDE(S) lease insert/refresh、旧EXCLUDEも期限まで保持 |
| CHANGE_TO_INCLUDE_MODE (3) | S空 | no-op、soft leaveとして旧lease満了を待つ |
| CHANGE_TO_EXCLUDE_MODE (4) | 全S | EXCLUDE(S) lease insert/refresh、旧INCLUDEも保持 |
| ALLOW_NEW_SOURCES (5) | S非空 | INCLUDE(S) lease insert/refresh |
| ALLOW_NEW_SOURCES (5) | S空 | no-op |
| BLOCK_OLD_SOURCES (6) | 全S | no-op。Queryを生成しないroleのため早期pruneしない |
| valid current Query | general/specific、S flag任意 | router lease insert/refresh、query decoded valuesをaudit、membership期限の短縮なし |
| valid old-version packet | old-version分類成功 | compatibility_floodをtrue（既にtrueならno-op）、Queryならrouter leaseもrefresh |
| effective expiry | key/generation/期限が一致 | 当該lease削除、filter再投影。空ならoverlay key削除 |
| stale wake | 三値不一致 | 状態不変、bridge timer audit stale_timer一件だけ、個別membership/dynamic control行なし |
| link down / VLAN unregister | 既存control成功 | leaseを保持、eligibilityだけfalse。再up後は未失効leaseを再利用 |
| invalid packet | 第3節違反 | whole-PDU control_drop、membership/router/epoch/転送copy変更0 |
| resource/arithmetic failure | reservation又はchecked演算失敗 | callback失敗、成功prefix維持、control_dropへ変換しない |
| run horizon T | event.time>=T | 発火なし、snapshotは成功prefix、未実績expiryを補完しない |

全source集合はIP byte昇順、Kはswitch/vid/family/group/port順。投影式：INCLUDE lease群の和集合I、EXCLUDE lease群E。E空ならINCLUDE(I)、E非空ならEXCLUDE((各E.sourcesの積集合)−I)。空IかつE空ならkeyなし。例 E1={a,b}, E2={b,c}, I={b} はEXCLUDE({})。この式は各leaseの受信predicateのORであり、host最新状態の正確復元ではない。旧希望の保守的保持による余分な転送は最大L、現在の正のreportは直ちに反映する。実際の全receiver到達を保証するには外部querier・report配送が必要である。

Queryの時間値はauditと安全fallbackに用いる。IGMP MRC/QQICのdecode8(c)=c (c<128)、それ以外(16+(c&15))<<(3+((c>>4)&7))。MRC単位100ms、QQIC単位1s。MLD MRCのdecode16(c)=c (c<32768)、それ以外(4096+(c&4095))<<(3+((c>>12)&7))、単位1ms。QRV=0は2、QQIC=0は125sを本roleのfallback値とする。general Queryで budget=RV×QI+response がL又はRを超える場合はcompatibility_flood=trueにする（期限を短いままsource-pruneし続けない）。これはquerier選出/標準router timer更新ではなく、本roleの安全策。specific Queryではbudgetを変更せずrouterをrefreshする。S flagは記録し、membershipを短縮しないことによりS=0/1とも同じ遷移になる。L/R以外のtimer、startup/last-member/retransmission timerは存在しない。乱数なし。

### 5.2 FES・atomicity・上限

時刻はu64 ps、[0,T)。phase0は既存完了・link/registration control→wire expiry→policy再投影、phase1でpacket arrival/decode/commit、phase2でqueue選択。phase1処理内から同時刻phase0 eventを予約しない。arrivalのmembership更新はphase1 callback内のNetworkDeltaで確定し、その後のphase2へ見える。同一時刻arrivalは共通FES seq順、PDU内はwire record ordinal順。expiryとrefreshが同時刻ならexpiry後に再insertするため二callback分のepochとなる。既存phase0 abstract batchの順序は変えない。

変化するstateはlease期限・filter・router・compatibility flagを含む。一callback内に実効変化があればpolicy_epochを一回だけchecked increment。同callbackの全control行は同じbefore/after epochを持つ。PDUで同じfilterが2回refreshされてもfinal staged mapを一度commitし、同期限の2件目はno_op。packet auditの成功は状態no_opでも残す。projectionが同じでも期限変更ならepochを進める。

generationはkey別ledgerで初回1、実効insert/refresh/expireごとにchecked +1し、削除後も保持する。再insertで1へ戻さない。compatibility flagは期限を持たないためgeneration=null。

期限indexはBTreeMap<(expires,key),generation>、更新で旧indexを除去し、bridgeごとnext wake一件だけをFESへ予約する。bridge wake token=(owner_bridge,wake_generation,scheduled_ps)、wake_generationは予約変更ごとchecked増分する。個別leaseのkey/generation/expiresはindexに保持する。取消済み/旧世代wakeはstate不変でtimer audit一件だけを残す。

有効wake tは、そのbridgeでexpires<=tの全index行を一つのstaged drainへ取り込む。正常進行では最小期限=tであり、t未満の取り残しはInvariantViolationとしてcallback失敗する。各行のkey/generation/expiresをlive leaseと照合し、一致行だけを削除する。対象をprotocol_revision→vid→group IP bytes（routerはnullで先頭）→port→mode（include,exclude,router）→source byte列の順に整列し、全削除後に各MembershipKey/RouterKeyを一度だけ再投影する。1 leaseごとのcallbackやepoch加算はしない。複数VID/port/familyを含んでもbridge drain全体で実効変化があればepochをちょうど1増分する。expiry後のnext wake予約、全audit、dynamic projectionを同じNetworkDeltaで予約・commitする。一件でも失敗すればdrain前state/FES/epoch/recordsを維持する。

有効drainはtimer audit一件＋変更のある(protocol_revision,vid,port) partitionごとにmembership audit一件を出す。各membership auditのchangesにはそのpartitionの全expired leaseを上記順に格納し、全行で同じepoch_before/afterを持つ。dynamic control auditは最終projectionが変わったKごと一件、変わらないKには出さない。例：同一port/groupのINCLUDE(a),INCLUDE(b)が同時失効するとtimer audit1、membership audit1（changes2）、dynamic membership_leave1、epoch+1となる。timer auditのexpired_leasesは2、affected_partitionsは1、projected_controlsは1。

グローバルpolicy_epochの相違だけで他keyの正当timerを無効にしない。epochは観測値、失効判定は対象generationで行う。この点は共通P-WIREの「古いepoch」を対象状態世代へ具体化したもの。未配送の取消済みtimerをrecord化しない。

有限packet workloadから全key/source/filter候補をprepareで走査する（正常decodeできるpacketのみ候補に含め、不正packetはdrop予定として保持）。転送copyは新規内容を生成しないので到達可能bridge/port積をchecked算出できる。key ledgerはmax_filter_leases/max_groupsを超える候補に対してprepare拒否し、実行中の未知key追加を許さない。総source保存量もmax_sources_total以内。上限ぴったり許可、+1は拒否。共通pending_timers/control records/copy IDs上限は既存budgetを共用。資源予約失敗時はWireState、DynamicState、FES、queue、ID counter、audit、epoch全てcallback前に保つ。正常drop auditの予約自体が失敗した場合もcallback失敗。

### 5.3 出力schema

schema2の `simulation.model_records` へ `ethernet.wire.membership/1` を追加予定。既存5ファイルと既存dynamic recordを改変しない。以下data fieldは全て必須、nullableは明記。未知field/enumをreaderは拒否する。u64/時刻/epochは正規十進文字列、countはu32 JSON整数。

| field | 型・意味 |
| --- | --- |
| effect_seq | u64文字列、commit order |
| packet_id | string又はtimer時null |
| protocol_revision | 上記enum |
| role | 上記固定role |
| switch / ingress_port | 既存ID string、timerも対象keyのport |
| vid / family | u16整数 / ipv4又はipv6 |
| time_ps / epoch_before / epoch_after | u64文字列 |
| outcome | accepted / control_drop / stale_timer / expired |
| reason | null又は ingress_ineligible / ip_length / envelope / ip_checksum / message_length / checksum / record_type / address / source_limit / destination_mac / unsupported_fragment / unsupported_extension / unknown_message |
| message_kind | query / report / legacy / timer / unknown |
| byte_length | u32、timerは0 |
| record_count | u32、decoded report M、query/timerは0 |
| query | null又は{group:string,suppress:bool,qrv:u8,qqic:u8,response_code:u16,response_ps:u64文字列,interval_ps:u64文字列}。旧queryはnull |
| compatibility_flood | commit後bool |
| changes | 下記delta array、drop/stale時空 |
| forwarded_ports | 重複なしport ID昇順array、drop/timer時空。候補であり送信完了実績ではない |

changeは `{key,lease_mode,sources,expires_before_ps,expires_after_ps,generation,action}`。keyは `{switch,vid,family,group,port}`、router/compatibilityはgroup=null。lease_modeはinclude/exclude/router/compatibility、sourcesはIPを4/16byteへparseしたunsigned network-byte昇順でsortした後canonical stringへ変換するarray（文字列辞書順ではない。192.0.2.2は192.0.2.10より前、router/compatibilityは空）。期限はu64文字列又はnull、compatibilityのgenerationはnull、それ以外u64文字列。action=insert/refresh/expire/enable_flood。no-op recordではchanges=[]。同一PDUのfinal diffを第5.2節のkey/mode/source byte順に出す。

bridge wake専用schema `ethernet.wire.membership_timer/1` の必須fieldは `effect_seq,switch,time_ps,scheduled_ps,wake_generation,outcome,epoch_before,epoch_after,expired_leases,affected_partitions,projected_controls`。時刻/seq/generation/epochはu64十進文字列、3countはu32 JSON整数、outcome=drained/stale_timer。staleは全count0・epoch不変。個別membership auditでstale_timerを捏造せず、このschemaだけへ出す。drain由来membership auditのpacket_id/queryはnull、message_kind=timer、byte_length/record_countは0、outcome=expired、reason=null。

投影されたmembership/router差分は既存DynamicControlRecordへkind=`wire_membership_set`/`wire_membership_leave`/`wire_router_set`/`wire_router_leave`/`wire_compatibility_flood`として記録する。control_idはcollision-free内部ID、scheduled_ps=applied_ps=callback時刻、batch_ordinalはkey順0始まり、generationは対象ledger世代、outcome=changed/no_op、keyとbefore/afterは既存SourceFilter DTO（compatibilityはbool）。fieldの既存型は維持する。before/afterの存在しない側はnull。projection no-opの場合dynamic control行は作らずwire auditのみ残す。wire auditとdynamic auditの両方を一transactionで予約し、一方だけ公開しない。Viewerはraw packetのlength/checksum結果とpolicy deltaを別表示し、forwarded_portsをcompleted transferに変換しない。

## 6. 検証条件と実装順

<a id="p-igmp-t01"></a>
**P-IGMP-T01**：IPv4 fixtureを独立checksum/byte-offsetで検査し、実装後はIGMP state transitions・expiry・転送・失敗atomicityを製品出力へ照合する。

<a id="p-mld-t01"></a>
**P-MLD-T01**：IPv6 fixtureをpseudo-header込みで検査し、同じ状態条件にIPv6/extension/RA検証を加えて製品出力へ照合する。

独立fixtureは [membership-wire-v1.json](../fixtures/membership-wire-v1.json)。byte vectorはL3全packet、各checksum期待値、decoded group/sourceを含む。state vectorは手計算集合/時刻であり、製品を実行して採取した値ではない。

| 必須検証 | 合格条件 |
| --- | --- |
| 正常6record type、zero source/M=0、複数records/同group反復 | §5.1の全行をIPv4/IPv6各々で実行、状態・audit・candidate一致 |
| aux/追加末尾/reserved/flags | checksumに含め、構文長内ならignoreして正常処理 |
| checksum bit反転・header checksum・pseudo-header source反転 | 対応reasonでwhole-PDU drop、成功部分recordも0更新 |
| countが残byte超過、unknown record、IPv4 MF/IPv6 Fragment、RA欠落/重複 | 対応reasonとcopy0、epoch不変 |
| 上限0/上限/上限+1、欠落/未知field、at+lease overflow | prepare又はcallback診断の区分が本書通り |
| EXCLUDE交差−INCLUDE和、ALLOW/BLOCK/TO_IN | 必要source即時追加、早期削除なし、個別lease満了時に再投影 |
| refresh→古いwake→実期限、同時刻expiry/report | staleは削除0、同時刻はphase0削除→phase1追加 |
| 複数port/VID/family、link down/up、static union | 他partition汚染0、eligibility/固定静的影響が一致 |
| old query/report、長いadvertised budget | compatibility_floodがtrue、以後run終了までflood、protocol生成0 |
| TX accounting・queued packet・T直前/同時/直後 | RX入力占有と転送TXを二重計上せず、[0,T)の実績だけ |
| 最終record/copy/FES予約への失敗注入 | state/FES/queue/journal/epoch/IDがcallback前hashと一致 |
| replay | snapshotと成功audit再生が一致、取消済み期限は実績に出ない |

実装順は (1) strict input/registry descriptor、(2) pure decoderとbyte tests、(3) lease planner/集合投影/上限tests、(4) NetworkDeltaと期限FES/epoch integration、(5) schema2/Viewer replay、(6) native CLIとfailure injection、(7)既存dynamic regression。各段階で失敗例を先に固定し、共通profileを未承認のまま公開しない。設計fixture検査の成功を(6)の代わりにしない。

```proposal-trace
{"nodes":[{"id":"P-IGMP-R01","stage":"requirement","upstream":[],"anchor":"p-igmp-r01"},{"id":"P-IGMP-F01","stage":"function","upstream":["P-IGMP-R01"],"anchor":"p-igmp-f01"},{"id":"P-IGMP-S01","stage":"spec","upstream":["P-IGMP-F01"],"anchor":"p-igmp-s01"},{"id":"P-IGMP-A01","stage":"architecture","upstream":["P-IGMP-S01"],"anchor":"p-igmp-a01"},{"id":"P-IGMP-D01","stage":"design","upstream":["P-IGMP-A01"],"anchor":"p-igmp-d01"},{"id":"P-IGMP-T01","stage":"verification","upstream":["P-IGMP-D01"],"anchor":"p-igmp-t01"},{"id":"P-MLD-R01","stage":"requirement","upstream":[],"anchor":"p-mld-r01"},{"id":"P-MLD-F01","stage":"function","upstream":["P-MLD-R01"],"anchor":"p-mld-f01"},{"id":"P-MLD-S01","stage":"spec","upstream":["P-MLD-F01"],"anchor":"p-mld-s01"},{"id":"P-MLD-A01","stage":"architecture","upstream":["P-MLD-S01"],"anchor":"p-mld-a01"},{"id":"P-MLD-D01","stage":"design","upstream":["P-MLD-A01"],"anchor":"p-mld-d01"},{"id":"P-MLD-T01","stage":"verification","upstream":["P-MLD-D01"],"anchor":"p-mld-t01"}]}
```

2026-10-17採用作業記録：本書のP-IDを[正式採用仕様](../../specs/後続拡張採用詳細機能仕様書.md)へ接続した。[現行API・scope](../../guide/reference/後続10分野の現行実装と利用境界.md)と[製品検証記録](../../verification/results/extension-product-2026-10-17.md)を参照。設計vectorと過去のdesign-only報告は履歴のまま保持し、製品合格とは区別する。文書版・履歴は本pushの改訂として確定した。
