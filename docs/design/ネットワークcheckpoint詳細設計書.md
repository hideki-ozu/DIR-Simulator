# ネットワークcheckpoint詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-network-checkpoint`
文書状態：基点からの追加実装。追加部分は未commit・未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。実行経路・版境界・検証範囲を規定 |

<a id="state"></a>

```trace
{"id":"design-network-checkpoint#state","stage":"design","requirements":["DIR-REQ-0280","DIR-REQ-0281","DIR-REQ-0282"],"upstream":["architecture#network-checkpoint"],"state":"confirmed","pending":[]}
```

`runtime/network/checkpoint.rs`が版1 owned-state codecを実装し、`registered.rs`がgeneric FESと接続する。u128は正規十進文字列で保存し、immutable scheduleは保存入力へ再bindする。GCLの消費済みcursorを含め、途中epochと同時刻残件を復元する。

検証はpending workからの参照確認と、live transfer/受信/制御状態から必要timerを逆算する両方向で行う。Completeはreception indexで検査する。必須Due/releaseを消して整合する件数へ偽装しても拒否する。bridgeとwire token、fragment完了とgap予約を対応させる。

authoritativeなNetworkArenaのrecordsはgeneric journalとは別に、選択profileのschema集合、H以下の時刻、object payload、descriptor検証とkey一意性を確認する。descriptor panicも公開前に診断化する。型のdeserialize成功を整合性検証の代わりにしない。

非boundary wire Packet入力の消費履歴は別台帳として保持しないため、その全pending発生数を過去から独立再構築したという保証はしない。通常の参照・時刻・timer意味検証と区別する。
