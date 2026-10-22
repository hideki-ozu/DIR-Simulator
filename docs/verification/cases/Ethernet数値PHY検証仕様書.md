# Ethernet数値PHY検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`verification-numerical-phy`
文書状態：未commit・未公開。project数値契約の検証範囲を示す。

<a id="dir-test-0142"></a>

```trace
{"id":"DIR-TEST-0142","stage":"verification","requirements":["DIR-REQ-0303"],"upstream":["design-numerical-phy#samples"],"state":"confirmed","pending":[]}
```


### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 実sample/byteと有限数値契約の証拠を規定 |

`tests/phy_channel.rs`の15件と`tests/phy_medium.rs`の17件で、FIR/ADC/noise、未知clock drift、複合反射/echo、lock/loss/retrain、3送信者の実collision/jam/BEB、duplex同時送受信、最後のchip後のIFG、動的admission、停止/checkpointと不正wakeを確認する。登録CLI例は`examples/ethernet/phy/numerical.ini`。32件の成功は宣言したprojectモデルの証拠であり、IEEE100/1000BASE-T1/T1S/PLCAまたは全電気適合の成功ではない。
