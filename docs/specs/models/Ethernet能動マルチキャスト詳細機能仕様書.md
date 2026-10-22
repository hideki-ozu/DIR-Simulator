# Ethernet能動マルチキャスト詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-active-multicast`
文書状態：未commit・未公開。本契約のfocused製品gate・独立レビュー済み。

<a id="protocol"></a>

```trace
{"id":"spec-active-multicast#protocol","stage":"spec","requirements":["DIR-REQ-0304","DIR-REQ-0305"],"upstream":["DIR-FUNC-0085"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 能動host/router/snooperの実packetと保存契約を追加 |

## 採用契約

`ethernet.network.composite.v1`の能動multicast設定で、IPv4はRFC9776のIGMPv3、IPv6はRFC9777のMLDv2を参照する。hostのsocket要求、interface全体のINCLUDE/EXCLUDE集約、状態変更report、query応答、再送、router/querier選出、旧版互換の期限を共通FESで処理する。受動packet parserだけで能動動作の完了としない。

query/reportは正しいIPv4/IPv6 header、Router Alert、TTL/Hop Limit、宛先、checksumを持つ実packetを生成し、既存のcontrol FIFO、出力port、媒体を通って相手とsnooperへ到着する。送信intentや予定arrivalだけで受信・membershipを成立させない。IPv4/IPv6が同じ物理ingressを共有しても、familyを照合してからdecodeする。

INCLUDEは指定sourceの和、複数EXCLUDEは受信関心の和として集約する。一つの下流portに複数のhostがいる場合も、後から届いたEXCLUDE listで前hostの関心を上書きしない。routerとsnooperは正のsource timer集合Xとblock集合Y、group timerを区別し、report種類・query・期限の遷移表に従う。source timer期限とgroup timer期限を単なるentry削除で代用しない。

低いアドレスのquerierを受信したら通常queryとpending last-member queryの送信を止め、期限経過後に再選出する。旧queryによる互換状態とsnooperの保守的floodには期限を設け、永続的なsource filter無効化にしない。受信General QueryのQRV/QQICから採用する変数もpacket値とlocal設定を区別する。

## timerと入力

全体schedulerはu64整数ps、query codeは各RFCの線形/指数形式を使う。ローカル設定とadvertise値を一致させるため、このprofileは単位とcodeへ正確に表現できる値だけを受理する。例としてQQICの200秒は`0x89`、MLDv2の40000ms responseは`0x8388`。200をそのままQQICへ書かない。旧MLDと現MLDでresponseのcode解釈を混ぜない。

interface、VID、port、MAC/IP、group/source数、再送、queryとresponseの時間関係をprepareで検証する。socket変更は版付き入力の順序と時刻へbindする。unknown source/filter、checksum破損、誤ったfamily/Router Alert/Hop Limitを黙ってmembershipへ変換しない。

## 継続と範囲

停止中にquery/reportを生成せず、保存した実socket prefix、集約filter、source/group/互換timer、querier、pending report/query、PRNG、snooperとowned wakeを復元する。stateの自己整合に加え、socketは実適用した変更prefix、wakeはactor内の次期限と共通FESへ照合する。

本契約は有限のhost/router/snooper実行モデルである。RFC全体の認証、IP unicast routingや任意のmulticast routing protocolを含めない。実装・修正・製品試験の完了範囲は実装台帳と検証仕様で別に判定する。
