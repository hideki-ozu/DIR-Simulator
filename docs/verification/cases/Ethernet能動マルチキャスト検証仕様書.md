# Ethernet能動マルチキャスト検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-active-multicast`
文書状態：v1.2.0収録。本契約のfocused製品gate・独立レビュー済み。

<a id="dir-test-0143"></a>

```trace
{"id":"DIR-TEST-0143","stage":"verification","requirements":["DIR-REQ-0304","DIR-REQ-0305"],"upstream":["design-active-multicast#protocol"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

## 受入試験

`runtime::network::multicast_active::tests`はpacket roundtrip/checksum、IPv4/IPv6のjoin/再送/source query、lower querierと期限復帰、dual-stack dispatch、旧querier期限、正確なfloating query code、last-member再送停止、socket prefixの意味改変を検証する。修正前に見つかったEXCLUDE集約とsource/group期限の不具合は、RFC遷移表に沿う追加試験で判定する。

`tests/network_active_multicast.rs`は実FIFOとlinkを使うjoin/source追加/leaveを受信実績で確認する。必須追加gateは、同じ下流portの二hostのEXCLUDE {A}/{B}、INCLUDEとの混在、source/group/旧版期限、query損失・選出、複数pending query、空のsource queryと本来のgroup queryの区別である。IPv4とIPv6双方で実packet配送の差を確認する。

`tests/network_checkpoint.rs`の別process継続は、report未送信、query待ち、受信後timer待ち、source filter/互換/querier期限を含む停止点を用いる。正しいrehashを対照とし、socketと集約filterの同時変更、cursor、pending timer、wakeを改変して拒否を確認する。actor内の形式検査だけを因果検証と扱わない。

## 現在の記録

最終actor25件、実製品2件、別process保存継続・改変4件が通った。group/source集約とexpiry、querier選出と喪失、旧版互換、複数query再送、空source query、checksum、同時刻Change/Timerの保存継続を対象とした。旧版TO_IN抑止をIGMPv1に限定し、IGMPv2/MLDv1とsnooperのsource leaseを追加確認した。実query喪失後のpruningは観測したが、喪失が追加のpruningを生んだとは扱わない。

[最終独立レビュー](../results/remaining-20261021/multicast-final-closure-review.md)で最後の互換性指摘を閉じた。規定したactor/物理統合の受入範囲であり、全RFC認証や履歴全体の真正性保証ではない。

最新CLIによるViewer確認は、PC/390px表示、前後ステップとsliderの往復、実記録だけのprefix、console/page errorと外部requestなしを対象にする。Viewerの成功をRFC group-state全体の合格に読み替えない。
