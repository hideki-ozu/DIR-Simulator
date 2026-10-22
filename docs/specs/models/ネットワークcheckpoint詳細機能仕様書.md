# ネットワークcheckpoint詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-network-checkpoint`
文書状態：基点からの追加実装。追加部分は未commit・未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。実行経路・版境界・検証範囲を規定 |

<a id="state"></a>

```trace
{"id":"spec-network-checkpoint#state","stage":"spec","requirements":["DIR-REQ-0280","DIR-REQ-0281","DIR-REQ-0282"],"upstream":["DIR-FUNC-0074"],"state":"confirmed","pending":[]}
```

対象はdynamic/TSN/CAN-Ethernet/Gateway+TSN/抽象FRER/抽象preemption/experimental wireの7 profile。汎用registered engineのコア状態に加え、NetworkStateとNetworkArenaを保存する。全queue、実行中transfer、受信、dirty journal、control予約、bridge変換、fragment/gap、wire protocol timer、Metric V2積分器と保存済みjournalを含む。

通常restoreは同一binary/build/lock、Registry、入力とcodec版を要求する。保存入力を専用InputSourceから再構築し、元ファイルの書換え・削除後もhost filesystemへfallbackしない。INI `metrics-version=2`のcatalogは保存入力から再構築して同一性を確認する。APIだけの有効化は有効化後の同一Registryを渡す。

decodeは独立arenaで参照・容量・予約・timerの全件対応を検証してから公開する。initialize、過去callback、取消timerを再実行しない。専用legacy engineのcodecや外部通信状態をこの対応範囲へ含めない。
