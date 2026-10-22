# Ethernetブリッジ構成詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-bridge-components`
文書状態：未commit・未公開。選択契約の実装・製品gate・独立レビュー完了。

<a id="bridging"></a>

```trace
{"id":"spec-bridge-components#bridging","stage":"spec","requirements":["DIR-REQ-0306","DIR-REQ-0307","DIR-REQ-0308"],"upstream":["DIR-FUNC-0086"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 共有LAN、MSTI、provider componentの追加実行契約 |

## 契約と参照版

IEEE 802.1Q-2022を参照し、既存の限定CIST契約を保持した追加構成として、共有LAN、MST領域とVLAN別instance、customer/provider componentを同一network ownerに結合する。状態機械、packet、実媒体、転送可否、FDB、保存継続を一つの実行経路で扱う。設定とcodecの存在だけでは実装完了としない。

共有LANは3つ以上のportを一つのMAC媒体へ接続し、一回の送信を他の受信portへ同じbytesで配送する。媒体はIFG解放まで一つの占有を持つ。これはブリッジ用の順序付きMAC serviceであり、数値PHYのcollision/jam/backoff契約とは区別する。RSTPはoperPointToPointMACに従い、shared designated portのagreementを誤って高速遷移へ使わず、shared root portの許容された高速遷移を保持する。同一bridgeの複数attachmentはBackup選出の対象とする。

MSTPは32byte以下の領域名、revision、4096-entry VLAN mapから算出するdigest、CISTと最大64のactive MSTIを持つ。CISTの7要素vector、MSTIの5要素vector、境界/Master、remaining hops、旧STP/RSTP受信とmigration、instanceごとのTCを処理する。共有LANの領域帰属は単純な隣接MCID一致だけで決めない。VLAN10と20を別MSTIへ割り当てた場合、実転送もそれぞれのtree policyを使う。

provider構成はC-componentのCEP/PEP、S-componentのCNP/PNPを区別する。CEPごとに専用のC-componentを設ける。物理portは既存NED出力portへ結合し、内部PEP/CNP handoffに架空の物理linkを作らない。外側S-TPID 0x88a8、内側C-TPID 0x8100の順序を保持し、S-componentは顧客C-tagをサービス内部の情報として保存する。学習と検索はcomponent、VID、MACで分離し、異なる顧客が同じC-VID/MACを使っても混線させない。内部PEP/CNPではS-tagを使わず、S-VIDはCNPのservice分類と物理PNP egressで適用する。3つ以上の顧客interfaceを入力制限で除外しない。C-componentは13.41のenhanced RSTPを使い、serviceのinterface数からPEPのoperPointToPointを導出する。

C-componentは顧客Bridge Group Address ...00を自分のprotocolへ渡し、provider ...08をfilterする。S-componentは顧客...00をservice内へ転送し、provider ...08を消費する。tag付替えは実bytes/FCS/MAC長を再生成し、送信時間とqueue容量へ反映する。priority-only VID0はport PVIDへ分類し、VID4095や所属外S-VIDを学習前に拒否する。

## 保存と可視化

共有媒体の一つのactive ownerと複数reception、component/instance別policy、領域identity、timer、tag stack、FDB scopeを保存する。予定到着から受信成功を合成しない。保存されたキー、参照、世代、占有とowned wakeを検証し、復元後も同じ実配送とbytesを生成する。Viewerは共有送信/受信、MSTID/境界、S-VID/C-VIDを区別する。

本仕様は実装の受入契約である。規範状態遷移の簡略化が残る場合は未完として記録し、IEEE全体の認証や未実行の製品試験成功を主張しない。

検証範囲と独立レビューは[拡張bridgeレビュー記録](../../verification/results/remaining-20261021/bridge-expanded-final-review.md)に記載する。
