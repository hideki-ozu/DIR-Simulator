# Ethernet能動マルチキャスト詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-active-multicast`
文書状態：未commit・未公開。本契約のfocused製品gate・独立レビュー済み。

<a id="protocol"></a>

```trace
{"id":"design-active-multicast#protocol","stage":"design","requirements":["DIR-REQ-0304","DIR-REQ-0305"],"upstream":["architecture#active-multicast"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 能動host/router/snooperの実packetと保存契約を追加 |

## ownerとservice

`runtime/network/multicast_active.rs`はschedulerを持たないactorであり、入力はStart/Change/Receive/Timer、出力は実IP bytesを持つ送信intentと次期限である。NetworkState/Deltaがactual FIFO admission、送信、共有媒体、受信、snooper反映とFESを所有する。IPv4/IPv6両interfaceへのdispatchは受信bytesのfamilyを先に検査する。

host socket mapとinterface集約filterを分離する。INCLUDE集合のunion、EXCLUDE集合のintersectionからINCLUDE要求を除いた値を使い、単独socketの更新で他socketの関心を失わない。状態変更の差分reportとqueryへのcurrent-state responseは別に管理する。

router/snooperのgroup stateはRFC9776の状態遷移表に対応するINCLUDE(A)またはEXCLUDE(X,Y)。Xは正timer、Yは0timerのsourceであり、EXCLUDE中のsource期限ではXからYへ移り、group期限ではINCLUDE(X)へ移る。IS_IN/IS_EXとTO_IN/TO_EX/ALLOW/BLOCKで集合・timer・query動作を区別する。空のsource queryを自動的にgroup queryへ変換せず、明示したgroup queryと分ける。複数の同時last-member queryのsource集合と再送を整合させる。

## queryと互換

General Queryから採用するQRV/QQIC、other-querier timer、startup query間隔、query response deadline、LMQTを明示的に保持する。自分がquerierでない間はpending last-member再送も実行しない。互換versionの期限が切れたら現versionのsource filteringへ復帰する。

`encode_float8`/`encode_float16`は閾値未満を線形、以上をmantissa/exponentに変換する。prepareは正確に表現不能な値を拒否し、暗黙の切上げでlocal timerとwireをずらさない。受信側は実packet codeをdecodeし、IPv6 pseudo-header checksumを含めて検証する。

## checkpoint

`validate_change_prefix`はimmutable Config.changesの実適用prefixからsocket mapと集約filterを再構築して照合する。`checkpoint_deadline`は未処理の同時刻期限も含め、次のactor deadlineと残る変更を求める。Network checkpointはsaved cursor、owned due ledger、FES timerを照合する。genuine rehash対照を通した後、socket/filterの同時改変やwakeの除去を拒否する。

group timersとlearned querier parametersを追加した場合は、actor構造だけでなく保存schema、不変条件、次期限と実受信に対する因果関係も検証対象にする。過去packet simulationを別に走らせて結果を合成しない。
