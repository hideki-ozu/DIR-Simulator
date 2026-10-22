# FRER規範回復検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`verification-frer-vector`
文書状態：未commit・未公開。本契約の製品gate・独立レビュー済み。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 選択契約の実装・検証と独立レビューの記録 |

<a id="dir-test-0147"></a>

```trace
{"id":"DIR-TEST-0147","stage":"verification","requirements":["DIR-REQ-0312"],"upstream":["design-frer-vector#vector"],"state":"confirmed","pending":[]}
```

`examples/frer-rtag/normative/model.ini`は通常FIFOと物理遅延を経た同一sequenceのcopyをtimeout前後に受信し、44イベントで実宛先へ2回配送する。実配送pointも2件であり、origin summaryの変更だけで成功と判定しない。

`tests/frer_normative.rs`の11件は、bridge/endpoint再配送、最初のsuffix喪失後の遅いcopy、複数suffixの同時伝搬、timeoutと受信が同時刻の場合、別processでの停止継続一致を確認した。deadline/generation/履歴/reset/生成器/round/入力欠落のrehash改変を拒否する。処理済みtimeoutを同時刻の後続deltaへ再予約する追加改変は修正前に受理されることを再現し、修正後は拒否した。

coreの5件は独立sequence境界vector、timer更新/非更新、instance分離、fanout/timer/配送の原子的予約失敗を含む。wireの5件はliteral FCS、予約bit、tagの有無、最小64byteからの除去を含む。既存FRER 10件も通り、省略algorithmの互換動作を確認した。Viewer16件と実製品結果の読み込み・逆方向replayも実施した。

本記録は選択契約のfocused製品gateである。最終独立レビューを完了した。リポジトリ全体の最終gateは継続中であり、機器全体のIEEE適合認証を意味しない。
