# TSNローカル時計詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-local-tsn-clock`
文書状態：未commit・未公開、製品gate実施中。

<a id="clock"></a>

```trace
{"id":"design-local-tsn-clock#clock","stage":"design","requirements":["DIR-REQ-0309","DIR-REQ-0310"],"upstream":["architecture#local-tsn-clock"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 独立node時計とTAS/受信PSFPの結合契約 |

## 既存ownerへの結合

`runtime/network/clock.rs`がnode時計と補正イベントを所有し、`runtime/ethernet/tsn/clock_projection.rs`でlocal cycleのgate状態と次のglobal wakeへ整数変換する。network ownerは既存のTAS送信hookと受信側PSFP hookへ同じ時計snapshotを渡す。補正イベントの順序と所有wakeをcheckpointに含める。

`TsnState::plan_arrival_clock_in`はPSFPのgate照会だけに`Option<&NodeClock>`を渡す。従来`plan_arrival_in`はNoneを渡すwrapperとして保持する。meterの`last_update_ps`、補充量、drop後token更新はglobal nowを使い続ける。clock-only compositeを有効な構成とし、PSFPを許可する一方、未実装組合せの検証を一括解除しない。

通常arrivalでは対向policy.portから受信deviceを特定し、bridge内部のsource arrivalではsource.deviceを使う。これにより送信側時計をPSFPへ誤って適用しない。最終出力は実policing verdict・受信とclock correction recordを関連付ける。
