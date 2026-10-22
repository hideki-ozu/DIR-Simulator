# Ethernetブリッジ構成詳細設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`design-bridge-components`
文書状態：v1.2.0収録。選択契約の実装・製品gate・独立レビュー完了。

<a id="bridging"></a>

```trace
{"id":"design-bridge-components#bridging","stage":"design","requirements":["DIR-REQ-0306","DIR-REQ-0307","DIR-REQ-0308"],"upstream":["architecture#bridge-components"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

## 既存ownerへの結合

NetworkState/NetworkDeltaと共通FESが物理送信、受信、有限queue、IFG、link状態を所有する。共有segmentは複数の独立linkとして並列送信せず、一つのreservationから受信先ごとのイベントを生成する。base dynamic bridgeからNED hubの論理転送を外し、hubでFDB学習やSTPを実行しない。parallel physical edgesは正確なgateの対でreverse方向を結合する。

WireStateはRSTP/MSTPのpacketとtimer stateを持つ。MSTPのphysical port contextとMSTID別tree stateを分離し、tree_policy(port,vid)をnetwork ingress/egress判定へ投影する。wire所有portは抽象tree再計算から外す。BPDU送信は既存control FIFOを通し、通常dataと同じ媒体占有を使う。policy変更とcomponent/VID別FDB flushは同じDeltaで公開する。

`runtime/network/provider_l2.rs`のProviderTopology、classify_ingress、encode_egress、handoff_allを実arrive/offer/arbitrateへ接続する。helperだけの別経路で成功を装わない。component FDBは実転送の正本とし、既存wire MAC tableとの二重所有を解消する。logical PEP/CNPからのhandoffは同じownerの論理遷移であり、実egressのみがMAC bytesと媒体時間を消費する。

ComponentTreeBindingは実deviceとは別にcomponentとcustomer/provider domainを持ち、PhysicalとInternal ISS attachmentを型で分離する。C-componentのCEPは一つで、PEP source MACはCEPのMACを用いる。protocol所有componentは同じcomponentのdata学習・転送・TC flushへtreeを投影する。S-tag付き顧客BPDUをEtherType serializerへ誤投入せず、Length/LLCと受信FCSを検証したenvelopeを再tagする。

## bytesと保存

EthernetWireFrameはlegacy tagと新しいtagsの両方を同時に持たない。untagged/単一C-tagの既存JSONを保持し、SまたはS+Cではtag:nullと順序付きtagsを用いる。実mac_hex、tag metadata、MAC長、FCSの一致を検証する。outer tagの追加/除去は既存retag serializerを使い、R-TAGや元のEtherType/LLC/payloadを失わない。

shared owner、per-instance policy、component FDB、logical handoff、actual wire queuesをcheckpointへ含める。nested tuple keyはJSON object文字列へ暗黙変換せず、正規化したentry配列で保存し、重複/順序/参照を検査する。schemaの更新は実状態の追加に合わせ、old profileの復元契約を黙って読み替えない。

## 現在の境界

共有data/control fanout、cycle/root喪失、MSTP境界、provider componentの実owner結合・複数PEP・TC flushを製品gateへ結合した。拡張したgateの最終独立レビューと統合再実行は完了した。詳細な残件は81項目の実装台帳で管理し、設計書のtrace経路成功を完了判定に使わない。

CIST/MSTI proposal同期ではforwardingのedge portを同期済みとして扱い、非edgeの未同期portを抑止条件として保持する。provider顧客BPDUのrelayはingressのlearningだけで許可せず、forwardingとlink_upも要求する。transport envelopeはimmutable prepared binding、実tag/VID/MSTID、component/ingress、Length/LLC/FCSと対応させ、再hashで別domainに読み替えた保存状態を拒否する。

検証範囲と独立レビューは[拡張bridgeレビュー記録](../verification/results/remaining-20261021/bridge-expanded-final-review.md)に記載する。
