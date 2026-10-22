# Ethernet数値PHY検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-numerical-phy`
文書状態：v1.2.0収録。project数値契約の検証範囲を示す。

<a id="dir-test-0142"></a>

```trace
{"id":"DIR-TEST-0142","stage":"verification","requirements":["DIR-REQ-0303"],"upstream":["design-numerical-phy#samples"],"state":"confirmed","pending":[]}
```


### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

`tests/phy_channel.rs`の15件と`tests/phy_medium.rs`の17件で、FIR/ADC/noise、未知clock drift、複合反射/echo、lock/loss/retrain、3送信者の実collision/jam/BEB、duplex同時送受信、最後のchip後のIFG、動的admission、停止/checkpointと不正wakeを確認する。登録CLI例は`examples/ethernet/phy/numerical.ini`。32件の成功は宣言したprojectモデルの証拠であり、IEEE100/1000BASE-T1/T1S/PLCAまたは全電気適合の成功ではない。
