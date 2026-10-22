# checkpoint版移行詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-checkpoint-migration`
文書状態：基点からの追加実装。追加部分は未commit・未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。実行経路・版境界・検証範囲を規定 |

<a id="migration"></a>

```trace
{"id":"spec-checkpoint-migration#migration","stage":"spec","requirements":["DIR-REQ-0294"],"upstream":["DIR-FUNC-0079"],"state":"confirmed","pending":[]}
```

`runtime::session::migrate_checkpoint`は通常restoreとは分離したoffline移行API。利用者が信頼するexact source identityとsource Registry、exact target identityとtarget Registry、名前/版付きtransform、保存上限を指定する。source/targetにはbundle schema、profile/result版、binary/build/lock hash、Registry fingerprint、codec catalogを含める。bundleから読み取ったhashだけを自動的に信頼登録しない。

旧Registryの実codecで旧stateを復号・意味検証し、transformへ所有state/blob/保存入力を渡す。変換後の保存入力をtarget validatorでprepareし、exact target identityと全stateを通常同等の厳密さで復号・検査してから新規bundleだけを公開する。initialize、イベント、finish callbackを実行しない。未知edge、不正参照、意味非互換state、adapter例外は診断とし、旧bundleと既存destinationを保持する。

移行中はcommit時刻/件数/last committed key、control/replay履歴とcounterを変更しない。将来FESやblobの変換は各edgeの責任で、target validatorが参照・tokenを再検査する。source manifestの実byte hash、run lineage、移行ID/版と前後identityをmigration_historyへ保存し、以後のnative checkpointにも引き継ぐ。

汎用APIが未知モデルの意味変換を自動生成することはない。各edgeは実旧実装によるfixtureとnative targetの継続結果で検証する。通常restoreの不一致拒否は保持する。
