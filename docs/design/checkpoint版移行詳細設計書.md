# checkpoint版移行詳細設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`design-checkpoint-migration`
文書状態：基点からの追加実装。追加部分はv1.2.0収録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="migration"></a>

```trace
{"id":"design-checkpoint-migration#migration","stage":"design","requirements":["DIR-REQ-0294"],"upstream":["architecture#checkpoint-migration"],"state":"confirmed","pending":[]}
```

`session/checkpoint/migration.rs`がexact edgeと変換payloadを定義する。`checkpoint_bundle::read_bundle_with_manifest_hash`はdescriptorから一度読んだmanifest byte列のhashを返す。pathを開き直してhashを取らない。source/targetのprivate restoreは同じ`restore_bundle`を使い、manifest/filesをborrowして全bundleの複製を避ける。

adapterへ渡すstateは正規JSON、filesはblobと保存入力である。古いstate.jsonの二重表現を除去し、最終typed stateから再生成する。`replace_codec`はcontent-addressed blobとcodec参照を同時更新し、別ownerが使うblobを消さない。変換例外をcatchして公開を止める。単にmanifestの版/hashを書換えて成功扱いにしない。

`encode_manifest`でfile path/hash/個数/総byte/blob/FES上限を先に検証する。targetの完全なprivate restoreが成功してもfinishを呼ばずに破棄し、その同じ内容を既存のno-clobber・manifest-last publisherへ渡す。履歴は隣接identity、lineage、正規hashを検査し、最後のtargetと現在identityを対応させる。

モデルの変換意味はtrusted adapterと個別fixtureの検証事項。構造検査だけで任意の変換stateが実際に到達可能だったことを数学的に証明する機能ではない。
