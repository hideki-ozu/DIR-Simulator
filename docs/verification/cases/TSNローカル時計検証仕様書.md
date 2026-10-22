# TSNローカル時計検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-local-tsn-clock`
文書状態：v1.2.0収録、製品gate実施中。

<a id="dir-test-0145"></a>

```trace
{"id":"DIR-TEST-0145","stage":"verification","requirements":["DIR-REQ-0309","DIR-REQ-0310"],"upstream":["design-local-tsn-clock#clock"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

## 判定と証拠境界

`runtime::ethernet::tsn::tests::psfp_local_gate_tracks_offset_drift_and_steps_without_refilling_tokens`で同じglobal時刻に対するoffset、有理数drift、負local値、backward stepを照合する。同時刻の±1000000000000ps補正を繰り返してもtokenは増えず、green→yellow→redになることを確認する。このunit gateは成功した。

`tests/network_psfp_clock.rs`は4つのINIを通常prepare/runで実行し、送信SOF0/2000000ps、実policing到着576000/2576000ps、identityの2件drop、offsetの2件pass、stepのdrop/passとendpoint受信件数を照合する。step recordは400000/1400000psである。通常3構成に加え、offset2^64の境界値でも実受信2件を確認して製品gateは成功し、実CLIでも16イベント、events_exhaustedを確認した。

補正前後とgate待ちで別process checkpoint継続を0/1/5/10イベントの停止点で照合し、一致した。最新binary由来のstandalone ViewerもPC/390px、前後stepとsliderの往復、console/page errorと外部requestなしを確認した。focused correctness reviewが検出したlocal時刻のu64縮小不具合は、wide周期位相と境界計算へ修正した。修正前の実CLI失敗を保存し、修正後の同一入力で14イベント、events_exhaustedと2件受信を確認した。TSN23試験、4構成の製品試験、低速有理数時計・wide境界の試験は成功した。修正後の独立レビューでwide位相127616と実失敗・成功の両証跡を再照合し、この範囲の指摘を解消した。gPTP wire同期の収束成功は本検証の対象結果に含めない。traceabilityのstrict成功は製品gateの代替にしない。
