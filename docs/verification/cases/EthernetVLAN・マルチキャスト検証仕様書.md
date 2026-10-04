# EthernetVLAN・マルチキャスト検証仕様書

文書バージョン：`1.1.1`
対象GitHubバージョン：`main @ 811360a`
予定公開版：`v1.1.1`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.1` | `2026-10-04` | 初版：独立wire期待値・VLAN/multicast経路・停止境界・Viewer・回帰の手順と製品試験証跡を記録。利用者指定の文書版1.1.1で初回push |

文書ID：`verification-ethernet-vlan`
文書状態：v1.1.1向けPR版。算術期待値に対する製品試験を実施。具体的な実施範囲・対象ソースは末尾と[実行記録](../results/ethernet-vlan-2026-10-04.json)で管理する。

<a id="dir-test-0102"></a>

## 1. DIR-TEST-0102：入力・wire・hop class

```trace
{"id":"DIR-TEST-0102","stage":"verification","requirements":["DIR-REQ-0224","DIR-REQ-0225","DIR-REQ-0228"],"upstream":["design-ethernet-vlan#ports-codec"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0056。[仕様](../../specs/models/EthernetVLAN・マルチキャスト詳細機能仕様書.md)の全fieldと分類表を確認する。製品serializerを使わない[独立wire fixture](../fixtures/ethernet-vlan/wire-vectors.json)でTCI、FCS、長さ、1Gbps直列化・占有を照合する。

| 入力・条件 | 期待値・観測 |
| --- | --- |
| payload0/42/45/46/47/1500byte、tagなし/あり | pad=max(0,46-P)、MAC長18+P+pad又は22+P+pad。empty64/68、max1518/1522。tag着脱でdata/padding不変・FCS再計算 |
| VID10、PCP7、DEI1 | TCI=f00a、headerのSA後に8100f00a。同一payloadのtagged/untagged FCSは別vectorへ照合 |
| empty、1Gbps | untagged直列化576000/占有672000ps、tagged608000/704000ps。propagationはEOF後、IFGはarrivalへ加算しない |
| source priority7 untagged→Switch default_priority2→tagged→Switch→untagged→Switch default_priority0 | source class7、第一・第二Switch class2、第三Switch class0。途中PCP2、source frame priority7は不変。DEIはuntagged再到達で0 |
| 同時候補でbyte上限64 | untagged64は受理、tagged68はqueue_full。source値64をtagged候補へ流用して受理しない |
| native PVID不一致のuntagged link | 各受信Port自身のPVIDへ分類し、hidden VIDを渡さない。結果はhopごとのVIDで説明可能 |

prepare負例を一箇所ずつ注入する：schema違い、未知/欠落/重複キー、非正規D、bool VID/PCP/DEI、VID0/4095、範囲外PCP/DEI、inner tag EtherType、1501byte、Port不足/重複/不正pair、PVID非member、複数untagged VLAN、他VIDのuntagged設定、表の重複キー・他Switch/非member egress、予約group・broadcast登録、source形式不一致・priority≠PCP、同flow source VLAN/tag不一致。generatorを残したtimes_ps:[]/count:0でもfieldを検証し、callback0の準備失敗を確認する。

admit不一致とingress membership不一致はvalid prepared topologyから実到達wireで発生させる。即filtered、ready=observed、planned_ready=null、候補copy0、理由一つを確認する。旧L2/QoSはschema3/tag/group MACを従来どおり拒否する。

<a id="dir-test-0103"></a>

## 2. DIR-TEST-0103：VLAN分離・group転送・複製

```trace
{"id":"DIR-TEST-0103","stage":"verification","requirements":["DIR-REQ-0226","DIR-REQ-0227"],"upstream":["design-ethernet-vlan#forwarding"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0057。すべてtree・1Gbps・固定遅延を明示した独立fixtureを作り、生成frame・全hop copy・receptionから以下を照合する。

| ケース | 構成と期待値 |
| --- | --- |
| 同MACのVLAN別FDB | SwitchにVID10→port B、VID20→port Cの同宛先MACを設定し、tagged VIDごとに指定一方向へ転送。Endpoint MACは引き続き一意とし、宛先と一致しない側のEndpointではfilterする |
| VLAN分離 | ingress VID10、B/CがVID10 member、DがVID20だけ。broadcast/未知unicastはB/Cのみ。Dへのcopy・queue_full・receptionを作らない |
| 既知unicastのingress指定 | same_ingress、候補0。floodへのfallbackなし |
| 登録multicast | VID10,01:00:5e:00:00:01→B/C、VID20同MAC→D。VID10ではB/Cのみ。Bが購読、Cが未購読ならB received/C multicast_not_subscribed |
| 明示空group/ingressだけ | multicast_no_egress、候補0。未登録扱いへ戻さない |
| 未登録group | Switch policy floodはeligibleだけ、dropはunknown_multicast・候補0。broadcastはこのpolicyから独立 |
| 複数Switch | group表を各Switchへ明示し、untagged/tagged枝の実wireから独立分類。購読だけで経路が自動構築されない |
| 部分満杯 | 登録groupのB queue容量0、C空。B drop queue_full、C転送・received、Switch forwarded。generated1、copy_dropped1、received1をそれぞれの母数で保持 |

具体的な混在fanoutの時刻例：A→S1はuntagged、S1→S2はtagged、S2→Bはuntagged。empty payload、各link delay1000ps、各Switch処理2000ps、Endpoint TX/RX処理0、競合なしとする。

| hop | SOF | EOF | release | arrival |
| --- | ---: | ---: | ---: | ---: |
| A→S1（64byte） | 0 | 576000 | 672000 | 577000 |
| S1→S2（68byte） | 579000 | 1187000 | 1283000 | 1188000 |
| S2→B（64byte） | 1190000 | 1766000 | 1862000 | 1767000 |

Bの完了受信1767000ps、serialization1760000、propagation3000、processing4000、queue_wait0。4成分の和を照合する。S2が同時にtagged出力Cへ複製する場合、C側EOF1798000、release1894000、arrival1799000psとなり、B/Cのwire・FCS・送信器は独立する。

<a id="dir-test-0104"></a>

## 3. DIR-TEST-0104：停止・結果・viewer・互換性

```trace
{"id":"DIR-TEST-0104","stage":"verification","requirements":["DIR-REQ-0229"],"upstream":["design-ethernet-vlan#observations"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0058。前節の具体例に対しT=576000/577000/579000/672000/1187000/1188000/1190000/1283000/1767000/1862000psとその+1で止め、Tと等しいイベントを除外する。C枝も1798000/1799000/1894000psとその+1を確認する。SOF/EOF/release/arrival/readyの実績と予定、未到達receptionなし、停止時のqueue・processingを確認する。

予約・算術失敗をGenerate（TX遅延0）、SourceReady（正TX遅延でOffer予約だけ）、Offer（正TX遅延のsource offer）、Arrival（処理遅延0）、Complete（正処理遅延）へ個別に注入する。Generateの失敗はframeも未公開、SourceReady/Offerの失敗は既存frameをunreadyに保持し、Offer/copy/queue効果を加えない。Arrivalの失敗はその到達・reception・候補を未commit、Completeの失敗は以前commit済みprocessing receptionを保持し、新しいready/候補/queue効果を加えない。event_seqは既存の予約IDを維持する。

frame/3・transfer/3・reception/2のschema集合、参照、tag/TCI/FCSとMAC長、CSV/JSON projection・manifest hash、metadata全Port/group/flow設定を確認する。bit量はcopyのwireに従い、filtered理由別総和、queue frame/byte、copy状態数、flow標本と期限母数、経路4成分を独立に照合する。途中停止の未完了を期限超過に数えない。

viewerはsource priority7とhop class0/2、VID切替、group/購読、tagged/untagged枝とFCS・長さ、理由、quiet Portをdesktop/mobileで表示する。VLAN表示filterでsummary母数を変えず、未到達を描画しない。>2^53ps seek、step前進/巻戻しの同じ到達先、連続線強調、offline出力、不正schema/参照/class/tagを検査する。

既存のCAN/GW、Ethernet L2 12fixture、QoS 11fixtureと4sampleを回帰する。profile別input・record schema・CRC・時刻・viewer結果を維持し、Rust fmt/clippy/locked tests、JS pure model/browser試験を実施する。

## 4. 実施状態と証跡の扱い

`2026-10-04`の開発ソースを対象に次の製品試験を実施した。対象ソース・binaryのSHA-256、実行コマンド、成果パスと判定は[実行記録](../results/ethernet-vlan-2026-10-04.json)へ保存する。公開済みタグの試験結果とは分けて管理する。

| 対象 | 実施内容と証跡 |
| --- | --- |
| 入力・runtime内部 | [入力テスト](../../../crates/dir-simulator/src/input/ethernet/tests.rs)3件と[runtimeテスト](../../../crates/dir-simulator/src/runtime/ethernet/tests.rs)5件。厳密field/値検証、空・未来負荷、旧profileのschema拒否、独立wire vector、copy別長さ・容量・分類、8種のfilter条件、Generate/SourceReady/Offer/Arrival/Complete失敗時のcommit境界 |
| public prepare/run | [VLAN統合テスト](../../../crates/dir-simulator/tests/ethernet_vlan.rs)7件。2sample、64→68→64byteの実時刻、変換後byte容量、VID別FDB・非member除外、既知/未知/空group・購読、priority7→2→0とDEI再生成、全4hopのSOF/EOF/release/arrival/readyのT/T+1、準備負例 |
| CLI・保存結果 | [unicast](../../../examples/ethernet/vlan-unicast.ini)と[multicast](../../../examples/ethernet/vlan-multicast.ini)をvalidate/run/view。[独立結果検査](../fixtures/ethernet-vlan/verify_results.py)でbit-wise CRC、TCI、copy長・予定/実時刻、分類・参照、到達保存則、理由別件数・link bit量、flow標本・期限母数・経路4成分、CSV projection、manifestのbyte数/SHA-256を照合 |
| Viewer | [pure modelテスト](../../../tests/ethernet_viewer_model.test.cjs)のVLAN fixtureでschema/tag/FCS/class/参照の負例、>2^53ps、queue bytes、quiet policyと表示filterを検査。[browser試験](../../../tests/ethernet_viewer_browser.cjs)で実生成した両sampleのdesktop/mobile/offline、Inspector、summary母数保持、step/巻戻しと連続線強調を確認 |
| 回帰・品質 | 既存CAN/GW、L2 12fixtureとQoS 11fixtureのRust試験、旧L2/QoS sampleとCAN/GWのbrowser試験。workspace fmt/clippy/locked tests、Viewer/NED editorのJS model、文書関連Python、strict traceabilityと生成物・diagram鮮度を確認 |

[wire fixture](../fixtures/ethernet-vlan/wire-vectors.json)とその検査scriptは設計式を標準ライブラリ・独立bit-wise CRCで確認する算術資料のままとし、製品合格記録には変更しない。製品serializerはその独立期待値へ照合し、生成結果も別の検査scriptへ渡す。上表は実施した試験範囲を示し、前節の条件の全組合せを網羅したという意味ではない。

性能、大量結果、IEEE適合認証、VID0/QinQ、動的登録・snooping・STP・TSNは未実施。文書traceの経路完備、wire算術検査、既存v1.1.0の試験記録と区別する。
