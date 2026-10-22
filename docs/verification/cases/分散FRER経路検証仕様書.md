# 分散FRER経路検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`verification-distributed-frer`
文書状態：未commit・未公開。本契約のfocused製品gate・独立レビュー済み。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 選択契約の実装・検証と独立レビューの記録 |

<a id="dir-test-0146"></a>

```trace
{"id":"DIR-TEST-0146","stage":"verification","requirements":["DIR-REQ-0311"],"upstream":["design-distributed-frer#paths"],"state":"confirmed","pending":[]}
```

製品fixtureは`examples/frer-rtag/distributed/model.ini`。source→replicationのprefix、独立した2-hop branch二本、elimination→destinationのsuffixを使う。baselineは物理送信6件、branch SOF 611000/1270000 ps、suffix SOF 1929000 ps、宛先処理完了2541000 psである。branchの74byteだけがR-TAGを含み、通常68byteのprefix/suffixへ復元する。

`tests/frer_rtag.rs`は実時刻、byte容量73/74境界、独立link喪失、通常traffic/branch共有出力の競合、途中horizon、suffix喪失と遅いcopy、最後のstale copyによる終端を確認する。10件成功。`network_checkpoint.rs`の2試験は13停止点の別process継続一致と、winner/phase/hop/sequence/bitmap/timeout/route/suffix親/deliveryの9種rehash改変拒否を確認した。

wire codec4件、原子的遷移1件は実tag除去/FCS/パディングと、二本目fanout予約・suffix予約の失敗を確認した。Viewer15件は実hop表示とrewind時の未来状態消去を含む。独立レビューで見つかった最後の拒否branchの終端とViewerの2件を修正し、再レビューを完了した。

これらは本placementとプロジェクトrecoveryの受入証跡であり、IEEE 802.1CBの全algorithmや未結合PHY/MAC Mergeの成功ではない。未commit・未公開であり、リポジトリ全体の最終gateは別に行う。
