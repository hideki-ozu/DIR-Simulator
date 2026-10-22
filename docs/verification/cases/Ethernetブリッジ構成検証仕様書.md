# Ethernetブリッジ構成検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-bridge-components`
文書状態：v1.2.0収録。選択契約の実装・製品gate・独立レビュー完了。

<a id="dir-test-0144"></a>

```trace
{"id":"DIR-TEST-0144","stage":"verification","requirements":["DIR-REQ-0306","DIR-REQ-0307","DIR-REQ-0308"],"upstream":["design-bridge-components#bridging"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

## 共有LANの製品gate

`tests/network_shared_lan.rs`と`examples/network-composite/shared-lan`を基に、3bridge、一回の媒体送信と複数実受信、data/control競合、IFGまでの占有を検証する。追加必須条件はloopのある代替経路、root/link喪失とtimer後の収束、FDB変化、同一bridgeの二portによるBackup、shared designatedの誤ったagreement shortcut拒否である。data-only fanoutの成功だけでN09を完了としない。

## MSTPの製品gate

Table13-2の独立digest3例、実BPDUのVersion3Length/64record境界とfallbackを照合する。同領域で異なるMSTI rootを設定し、VLANごとの実data経路が分岐すること、map/name/revision差で境界になること、RSTP/旧STP相手へのCIST動作、Masterとremaining hops、instance限定TC/FDB flushを検証する。共有LANで同じMCIDの二bridge間が別領域のdesignated bridgeに隔てられる規範例も対象にする。helper内のrole比較だけを製品経路の証拠にしない。

## providerの製品gate

`examples/network-composite/provider-l2`は2つのserviceでC-VID10と同じ送受信MACを再使用する。data-only互換fixtureではeast顧客Aに3CEPを配置し、内部PEP/CNPをNED物理wireと分離する。a_data/a1_data/a2_dataはa3、b_dataはb1へ届き、別serviceへ漏れないことを実receptionで判定する。PNP bytesのS100/S200+C10、PCP/DEI、payload、FCS、MAC長と媒体時間を独立期待値と照合する。customer egressは元C-tagへ復元する。

protocol.iniはCEPごとに専用C-componentを設け、6つのC RSTPと2つのS MSTPを別ownerに割り当てる。S100は4interface、S200は2interfaceである。内部ISSにはS-tagを載せず、PEPのpoint-to-pointは実service cardinalityから導出する。このprotocol有効variantで顧客BPDUのservice通過、provider BPDUのS-component消費、C-componentによる...08 filter、C/S別tree、component別TC flush、3interface時のshared PEP動作を検証する。VID0、VID4095、membership外S-tag spoof、queue満杯、handoff中断と二重送信なしも負例に含める。

## 継続・出力gateと現在の状態

data/control待機、共有送信中、BPDU収束途中、TC/timer待ち、stacked tag付き送信中で別process checkpointを行い、無停止結果の全bytes/eventと一致させる。正しいrehash対照の後、segment owner、component/VID/MSTID、tag metadata、timer/wakeを改変し、復元のatomic拒否を検証する。Viewerは最新binary由来の結果をPC/390pxで開き、前後step/sliderの往復、console/page errorと外部requestなしを確認する。

VLAN別dynamic投影29件、出力validator5件、shared data製品2件および共有送信中の別process再開は成功した。MSTP Backupの実BPDU観測も得られた。循環したMST領域の製品試験ではVID10/VID20が異なる実経路で一回ずつ届き、CIST-blocked/MSTI-forwarding portでのMAC学習と元bytes保持を確認した。追加した境界・RSTP peer・共有root故障の製品gateは後述のとおり成功した。拡張gateを含む最終独立レビューは完了し、保留していたcurrent-sourceのMSTP基本・境界gateも再実行して通った。

### provider実配送の追加記録

現在のowner結合で、元fixtureを変更せず116イベント、意図したendpoint受信6件、PNP送信6件、内部handoff12件を確認した。2つの顧客はC-VID/MACを再使用しても配送先が分離された。独立[byte/FCS/time oracle](../fixtures/provider-l2/verify_result.py)はimmutable workloadからS/Cヘッダとpayloadを組み立て、Python zlibのCRC32、実MAC長72byte、1Gbpsの送信640000psとIFG96000psを6件照合して成功した。

同じ結果由来のstandalone ViewerはPC/390px、前後stepとslider往復、console/page errorと外部requestなしを確認した。別process checkpointも0/1/20/50イベント停止点で継続一致を確認した。これらはdata serviceと保存継続の証拠であり、providerのBPDU境界、3interface時のenhanced RSTP、TC/FDB無効化、改変checkpoint全gateの完了ではない。

VLAN別dynamic投影は既存分を含む29試験に成功した。追加製品fixtureは`examples/network-composite/mstp-region`であり、CISTでは閉じるportを通るVID10と、直接路を通るVID20を実byte/受信/学習で区別する。この製品gateは成功し、規範全体の残件は別に継続している。

### 共有root喪失とcomponent制御の追加記録

`tests/network_shared_tree.rs`はMSTPとRSTPで実cycle/root喪失を検証した。RSTPではs2の継続中Root/Designated port上のentryを消去対象と誤認しない。35sの逆向きprobeがs1.tx_bに学習した非static entryを、36sのlink喪失に伴う規範port限定flushで実際に消去する。元の故障前後の2配送、Backup、媒体占有と実bytesの検証も保持した。MSTP gateと、修正後のRSTP gateはそれぞれ成功した。

component制御ではLearning=true/Forwarding=falseのingressから顧客BPDUをrelayしないことを実plannerで確認した。実際のphysical relayがqueueへ入る時点のcheckpointを別processで継続し一致した。再hashしたtransport欠落、local protocolへの偽装、component、ingress、VID、MSTIDの6改変は意味検査で拒否した。これは全component causal-history proofの完了を意味しない。

### 境界・複数PEP・実TC flushの最終追加gate

`tests/network_mstp_boundary.rs`はregion名、revision、VID→MSTID mapの差とRSTP peerの4構成で、VID10/VID20の実byte配送と物理parent chainを確認した。修正前にはedge portのforwardingがproposal同期を妨げて配送が停止した。CIST/MSTI双方の同期判定でedge portを規定どおり扱い、4構成とMSTP unit18件、既存製品1件が通った。IEEE 802.1Q-2022 §13.7.1に従い、RSTP peerはMSTP v3を受信できる。MST bridgeへ自動v2送信を要求せず、peer自身の実v2送信と受信を確認する。

`tests/network_provider_multi_pep.rs`はeast.caとwest.cbに2本目のPEPを追加した。元の6配送にC20→S300の双方向配送を加え、8宛先配送、8PNP横断、26物理copyを確認した。元のunknown-unicast学習に伴う正当なfiltered copyを除外して件数を合わせない。全FCS、inner C20とouter S300、顧客MACへの復元を独立CRCと照合した。

現在のprotocol全体実行は1422イベント・13411記録・実配送6件で完走した。顧客BPDUのservice relayはForwarding成立後の4sから始まる。追加の`tests/network_provider_topology.rs`は40sに実学習し、40.5sにPNPを切断する。300sのage期限より前に、Net.east.sのS100/S200別MAC entryを実wire topology-change効果で消去した。別種類の管理controlは`/controls/0`で拒否する。追加gate1件が通った。

最新protocol結果とmulti-PEP結果のViewerは1440px/390px、記録の逆方向replay、page errorなし、外部requestなしを確認した。これらを含む最終独立レビューは完了し、新しいP1/P2の阻害要因は認められなかった。

独立レビューと統合再検証の範囲は[拡張bridgeレビュー記録](../results/remaining-20261021/bridge-expanded-final-review.md)に記載する。
