# EthernetVLAN・マルチキャスト詳細設計書

文書バージョン：`1.1.1`
対象GitHubバージョン：`main @ 811360a`
予定公開版：`v1.1.1`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.1` | `2026-10-04` | 初版：Port policy、copy別wire/class、原子的VLAN/group交換、schema・Viewerの実装と検証への対応を記録。利用者指定の文書版1.1.1で初回push |

文書ID：`design-ethernet-vlan`
文書状態：v1.1.1向けPR版。既存Ethernet Engineへ以下の変更を実装。製品試験の範囲は[検証仕様](../verification/cases/EthernetVLAN・マルチキャスト検証仕様書.md)で管理する。

## 1. 既存構成への追加

v1.1.0の`EthernetWireFrame`は元frameで不変、runtimeのoffer・queue_point・Start・Arrivalとoutputのbit集計は元frameのwire/priorityを参照している。VLANでは同じ元frameから異なる長さ・FCS・classのcopyが生じるため、これらを一体で変更する。[詳細機能仕様](../specs/models/EthernetVLAN・マルチキャスト詳細機能仕様書.md)を入力・出力契約の正本とする。

新しい汎用runtimeを並立させず、既存Ethernet Engineのprofile分岐とtyped payloadを拡張する。非VLAN profileは現在のserializer・FDB・入力拒否・出力projectionを維持する。

<a id="ports-codec"></a>

## 2. prepareとwire表現

```trace
{"id":"design-ethernet-vlan#ports-codec","stage":"design","requirements":["DIR-REQ-0224","DIR-REQ-0225","DIR-REQ-0228"],"upstream":["architecture#arch-ethernet-vlan"],"state":"confirmed","pending":[]}
```

### 2.1. 型と所有

| 型・所有先 | 追加する内容 |
| --- | --- |
| `EthernetVlanTag` | native整数のvid:u16、pcp:u8、dei:u8。入力検証後に構築 |
| `EthernetPortPolicy` | canonical tx/rx、所有device、pvid、admit enum、default_priority、BTreeMap VID→tagged。tx/rxは共有NEDのpaired pathsで解決し、文字列suffixだけで推測しない |
| `PreparedEthernet` | 選択profile、Port policy、Switch別(VID,MAC)→egress index/egress集合、Endpoint group集合、source VLAN。既存方向・output・generatorの所有を再利用 |
| `EthernetWireFrame` | optional tagを追加し、data/padding/header/FCSを一貫した不変値で保持。基本版serializerの公開APIをwrapperとして維持 |
| `EthernetTransferRecord`/待機copy | wire、vlan_id、effective priorityを所有。source copyも明示保持。容量drop候補も同じ表現 |
| `EthernetReceptionRecord` | incoming transferから分類したvlan_id・priority。processing状態でも到達したwireから求めた値だけを保持 |

prepareはschema3をVLAN profileだけへdispatchし、全接続Portとoutputsを検証、MACを既存の小文字表現へ正規化する。FDB/group/購読キー、egress集合、Port membershipをBTreeMap/Setで重複検出して辞書順へ整列する。profileの追加は`input.rs`、`input/can.rs`のprofile判定、`runtime.rs`、`tool/viewer.rs`とshared viewer loaderのallowlistも含める。

source workload/tagをcacheする前にsource policyとflow一貫性を検証する。空列・count=0・将来候補でも省略しない。既存のu128候補計算・u64実行時刻・終了境界を変更しない。

### 2.2. 変換と容量・時刻

`serialize_source(payload,tag)`はdataに46byteまでzero padし、指定headerとFCSを構築する。`rewrite_for_egress(incoming_wire,classified,policy)`はdata/paddingを保持し、tagをmembershipの形式へ変換、FCS再計算、MAC長を検証する。各値は`wire.mac_hex`の実byte数・header・CRCへ照合する。単一tagしか生成しない。

offer候補は`{direction,parent_transfer_id,wire,vlan_id,priority}`。queue admissionはcandidate.wire.mac_bytes、取出しは実copyの同じ長さを減算、queue pointはcandidate/copy.priorityへ紐付ける。元frame値を使う残存経路を検索して除去する。

StartはcopyのMAC長Mを使い、`W=8*(M+8)`bit、`O=W+96`bit。`EOF=SOF+ceil(W*10^12/bitrate)`、`release=SOF+ceil(O*10^12/bitrate)`、`arrival=EOF+delay_ps`。既存のchecked演算で全予定時刻をpreflightする。SOF時にEOF/release、EOF時にArrivalを予約する既存境界を保持する。IFGは受信完了へ加算しない。

ArrivalのFCS・受信長検査はincoming transfer.wireを使う。元frameとwire差分があること自体を不変条件違反にしない。Source wireと異なるcopyでもdata、src/dst MAC、inner EtherType、paddingは不変であることを検査する。

<a id="forwarding"></a>

## 3. ingress判断と交換のcommit

```trace
{"id":"design-ethernet-vlan#forwarding","stage":"design","requirements":["DIR-REQ-0226","DIR-REQ-0227"],"upstream":["architecture#arch-ethernet-vlan"],"state":"confirmed","pending":[]}
```

Arrivalでincoming wireを検査・分類し、admit→membership→Endpoint宛先適合を判定する。即filterのreceptionはready=observed、planned_ready=null。入場した受信はprocessingとしてSwitch forward_delay又はEndpoint rx_processing_delayへ渡す。ゼロ遅延は既存同一callbackで完了し、正遅延はphase0通知からphase1完了へ進める。

Switch完了時には(VID,MAC)参照とeligible集合から全候補を求める。known groupの空集合とunknown lookupを区別する。各Portのegress形式で独立wireを構築する。tagged->untaggedでもそのSwitchのoutput priorityは受信classを維持し、次の受信側は再分類する。DEIもuntagged linkで失われる。

```text
incoming wire -> classify(VID,priority,DEI) -> admission
  -> processing -> VLAN lookup -> sorted candidate egresses
  -> per-copy rewrite -> preflight capacity/ID/bytes/events
  -> commit reception + all transfer outcomes + queue effects + reservations
```

phase0完了/解放→phase1入力/offer→phase2Startを維持する。candidate全体でID重複、byte加算、dirty方向・event_seq容量、journal参照を検査し、成功後だけstateを更新する。個別queue_fullは正常なcandidate結果であり、batch停止ではない。

ゼロ処理遅延のArrivalでこのbatchが失敗した場合、そのcallbackのarrival milestone・reception・新copy・queue pointも未commitにする。正処理遅延のCompleteで失敗した場合、以前commitしたArrival/processing receptionは残すが、ready/forwarded・新copy・queue変更を加えない。

source TX遅延0ではGenerateがframeとsource offerを一体でpreflightする。正TX遅延ではSourceReadyはphase1 Offerの予約だけを担当し、予約失敗時は既存frameをunreadyのまま保持する。続くOfferでframe readyとcopy/queue効果を一体でpreflightする。これらのcallback境界を維持し、finishで不足行を補わない。

connected treeのままなので`frame_id@from_port`とincoming transfer parent関連を維持できる。同一frameによる同一output再訪が起きた場合は内部不変条件違反とし、IDの付番追加でループを許容しない。

<a id="observations"></a>

## 4. projection・集計・viewer

```trace
{"id":"design-ethernet-vlan#observations","stage":"design","requirements":["DIR-REQ-0229"],"upstream":["architecture#arch-ethernet-vlan"],"state":"confirmed","pending":[]}
```

`output/ethernet.rs`はVLAN profileに限ってframe/3、transfer/3、reception/2を登録し、同じtyped snapshotからCSV/JSONへprojectionする。旧profileのrecord dataへtagやwireを追加しない。metadataの全静的設定、source flow contract、schema集合をcanonical writerと既存manifest-last公開へ渡す。

link量は各transferのwire長、payload量は不変data長を使い、EOF/releaseの実績時刻と[0,H) clippingを継承する。filter理由別件数はreceptionの最終状態から一回だけ集計する。`filtered=sum(reason counters)`、copy件数・queue bytes保存則、完了受信の4成分和を不変条件とする。現在のflow分析はparent chainの実績区間を使うため、wire長の変化を固定長へ置き換えず継承できる。

viewerのpure Ethernet modelでprofile/schema集合とTag値・wire hex長/FCS・parent/方向・分類・queue classを検証する。受信tagとreception分類はそのingress policyへ照合する。source VLANがhop VLANと異なるだけで拒否しない。typed validated modelをethernet-appへ渡し、source指定とhop class、転送済み/filtered/drop/未完了を別々に表示する。VLAN選択で全体summaryを再計算しない。

再生はBigIntと実績時刻から毎回復元する。前進/巻戻しの到達先区間を同じcopyで再演し、連続再生は方向線を強調する。表示上限、省略件数、quiet topology、offline assetsを既存方式で保持する。CAN loader/modelは既存試験で回帰確認する。

<a id="implementation-order"></a>

## 5. 実装する順序と完了条件

| 順 | 実装範囲・主なファイル | 次へ進む条件 |
| --- | --- | --- |
| 1 | schema3とpolicy/tagの型、prepare・serializer。`input/ethernet.rs`、`lib/types/ethernet.rs` | 入力正常/拒否、VID境界、CRC/長さの独立vector、旧profile拒否条件が一致 |
| 2 | hop-local copy、容量・時刻・FCS。`lib/snapshot/ethernet.rs`、`runtime/ethernet.rs` | 64/68byteの分岐、priority再分類、MAC byte上限・非プリエンプト、原子的失敗が一致 |
| 3 | VLAN別FDB/flood/group/購読、停止prefix。同runtime | VLAN分離、同MACのVID別経路、既知/未知group、空集合、部分drop、途中停止が一致 |
| 4 | schema/metric/metadata、viewer、CLI・offline出力。`output/ethernet.rs`、`output/ethernet/flow.rs`、viewer assets・dispatch | CSV/JSON/hash、理由保存則、desktop/mobile、step対称性とCAN/L2/QoS回帰が一致 |
| 5 | `examples/ethernet`の実行可能例と製品実行記録、説明書 | validate/run/viewで独立期待値と照合し、Rust fmt/clippy/locked tests・JS/browser・文書検査を実施 |

実装前の算術fixtureは[wire vectors](../verification/fixtures/ethernet-vlan/wire-vectors.json)へ置く。[unicast例](../../examples/ethernet/vlan-unicast.ini)・[multicast例](../../examples/ethernet/vlan-multicast.ini)をCLIで実行可能にした。入力・Rust内部境界・外部CLI・viewerの[製品実行記録](../verification/results/ethernet-vlan-2026-10-04.json)を別途保存し、既存v1.1.0の証跡をVLAN合格へ流用しない。

この段階が完了した後の次候補はCAN↔Ethernet Gateway。変換ルール、lineage、サイズ/優先度対応、複数受信・待機容量を別途設計し、VLAN表や静的group設定へ暗黙に混在させない。
