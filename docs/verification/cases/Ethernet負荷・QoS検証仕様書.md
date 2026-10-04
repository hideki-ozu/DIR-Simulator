# Ethernet負荷・QoS検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 45ce163`
予定公開版：`v1.1.0`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-04` | 初版：QoS独立fixture・製品試験、viewer・CAN回帰、停止境界の実施範囲と証跡 |

文書ID：`verification-ethernet-qos`
文書状態：開発版。以下の独立期待値を製品実行と照合済み。実施範囲は末尾に記録する。

<a id="dir-test-0099"></a>

## 1. DIR-TEST-0099：負荷cursorと入力互換

```trace
{"id":"DIR-TEST-0099","stage":"verification","requirements":["DIR-REQ-0218"],"upstream":["design-ethernet-qos#workload"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0053。periodic start=0,phase=10,period=2000000,count=3から[10,2000010,4000010]psを独立に求める。burst start=10000000,period=2000000,burst_count=2,n=2,spacing=100から[10000000,10000100,12000000,12000100]psを求める。end/Tと等しい発火を除き、0件、同時発火、空explicit、入力配列反転、u64付近の候補を照合する。

未知・欠落・重複キー、bool priority、範囲外priority、period=0、n=0、overlap burst、非正規D、同flow設定不一致を一箇所ずつ注入し、準備失敗とcallback0を確認する。v1は追加kind/fieldを拒否し、既存12fixtureの結果・CRCを保持する。

<a id="dir-test-0100"></a>

## 2. DIR-TEST-0100：class選択・容量・非プリエンプト

```trace
{"id":"DIR-TEST-0100","stage":"verification","requirements":["DIR-REQ-0219","DIR-REQ-0220"],"upstream":["design-ethernet-qos#queues"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0054。1Gbps、empty payloadのMAC64byte、wire576bit、占有672bitを用いる。同時刻にID辞書順でpriority0/7/3をofferし、strictでは7→3→0、FIFOでは0→7→3、各SOFが0/672000/1344000psとなることを照合する。

byte容量127へ同時刻のMAC64byteを3件offerし、1件受理・2件dropを確認する。容量64・frame容量1へ[0,1,2]psでofferし、送信中を除いて2件目が待機、3件目がdropとなることを確認する。MAC1518byteを容量1517へofferしてdrop、byte容量0、frame容量0、port合計上限、class間独立、fanout部分満杯も確認する。

低優先度1500byteを0ps、高優先度emptyを1psで生成し、低優先度のEOF12208000/release12304000psまで高優先度が割り込まず、sourceの高優先度SOF12304000psを確認する。同時刻解放→offer→送信選択、全candidateの予約失敗時の未公開、class/port件数・byte保存則を照合する。

<a id="dir-test-0101"></a>

## 3. DIR-TEST-0101：フロー標本・内訳・viewer

```trace
{"id":"DIR-TEST-0101","stage":"verification","requirements":["DIR-REQ-0221","DIR-REQ-0222","DIR-REQ-0223"],"upstream":["design-ethernet-qos#observations"],"state":"confirmed","pending":[]}
```

対象AC：DIR-AC-0055。基本3Endpoint+Switch、link delay1000ps、switch処理2000ps、empty frameの最初の完了受信は1156000ps。内訳queue0、serialization1152000、propagation2000、processing2000psの和へ照合する。キュー競合・Endpoint TX/RX処理を追加し、各成分の増分を確認する。

受信標本0/1/複数、nearest-rank分位値、最大−最小jitter、期限より前／等しい／後、期限null、途中停止、未知宛先filter、broadcast複数受信、copy dropを確認する。未完了を期限超過や完了受信へ数えない。flow全体とreceiver別の件数・標本数を関連IDから照合する。

CSVとJSONのprojection一致、manifest hash、profile別schema集合とmetric unit、静かなEndpointの初期stateを検査する。viewerはdesktop/mobileで方向別同時送信・class待機・詳細選択・flow表、step前進／巻戻しの同一到達先表示、連続再生で線だけの強調、>2^53psの正確なseek、不正参照・不正版・予定だけの受信を検査する。既存CAN model/browser試験を回帰として実行する。

## 4. 実行記録

実行結果を、入力・対象作業ツリー・実行環境・独立期待値・実績・判定・成果パスとともに記録する。仕様経路の整備と製品試験の合格はそれぞれ管理する。

| 日付・対象 | 実施範囲と証拠 |
| --- | --- |
| 2026-10-04・負荷／QoS製品 | [入力fixture](../fixtures/ethernet-qos/scenarios.json)とRust外部結合試験`ethernet_qos`の5試験で、FIFO／strict priority、MAC byte／合計frame容量、非プリエンプト、周期・バースト、期限・分位値・空／単一標本・fanout copy破棄を照合。[CLI実行証跡](../results/ethernet-qos-2026-10-04.json) |
| 2026-10-04・停止と内部境界 | `ethernet`の6外部試験と内部Ethernet試験でEOF／release境界、実績だけの受信、予約失敗の未commit、reservation event_seq、拒否offerの不変占有、u64範囲外の論理候補を確認 |
| 2026-10-04・viewer | Ethernet純粋モデル10試験と実L2／priority／周期・burst／flow結果のChromium desktop/mobile確認。CAN model23試験・既存CAN browserも回帰確認。詳細は[統合検証記録](../results/ethernet-integration-2026-10-04.json) |
| 未実施 | 性能・大量結果、IEEE適合認証、媒体v2、VLAN／multicast制御、CAN↔Ethernet変換、TSN、公開Registry/Envelope。これらの文書経路を第一段階の製品適合と取り違えない |
