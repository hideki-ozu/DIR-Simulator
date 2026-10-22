# checkpoint版移行検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`test-checkpoint-migration`
文書状態：基点からの追加実装。追加部分は未commit・未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。実行経路・版境界・検証範囲を規定 |

<a id="dir-test-0136"></a>

```trace
{"id":"DIR-TEST-0136","stage":"verification","requirements":["DIR-REQ-0294"],"upstream":["design-checkpoint-migration#migration"],"state":"confirmed","pending":[]}
```

`tests/checkpoint_migration.rs`は実Model実装v1（legacy_count/next）で保存し、別Model実装v2（completed/audit_xor/timer）へ移行する。default restoreによる版不一致拒否、旧codec→新codecの復号、native v2との継続結果一致、後続checkpointへの履歴維持、source byte不変とdestination非上書きを検証する。

不正edge、誤target、H変更、last_committed_keyだけの変更、不正journal、孤立timer、adapter panic、hashは正しいが意味不正な旧stateを拒否し、失敗時のdestination未作成を確認する。移行でinitialize/event/finishが呼ばれないことも実callback記録で確認する。

最初の3 testは合格し、独立Astra reviewでlast_committed_keyの保護漏れを検出・修正した。該当negative追加後も移行3件・既存control/checkpoint45件・NoC9件の統合再実行が合格した。このfixtureは同一binary内の実旧/新モデル・codec版であり、別の歴史的実行ファイル間の移行成功とは区別する。別binaryのCAN v1 production fixtureも保存済みで、CAN v2対応edgeの検証を別に行う。
