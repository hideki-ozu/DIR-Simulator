# MAC Merge検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-mac-merge`
文書状態：v1.2.0収録。選択した静的ideal-clock契約の製品検証・独立レビュー完了。

<a id="dir-test-0148"></a>

```trace
{"id":"DIR-TEST-0148","stage":"verification","requirements":["DIR-REQ-0313"],"upstream":["design-mac-merge#merge"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

core/原子的遷移18件と製品経路11件が通った。全addFragSize、100Mb/s/1Gb/s、実LLDPとV/R、response喪失による3回試行、同時刻Respond優先、link reset、legacy D5 fallback、実R-TAG branch、別process継続と意味改変を含む。独立CRC計算で実captureの3fragmentすべてを照合した。

実pair CLIは55イベントで3frameを配送し、最終RXは7264000ps、releaseは7360000psだった。final fragment喪失variantはexpressのみ2frameを配送し、slow frameの配送は0、RX組立は120octetで未完となった。喪失packetのactual arrivalはnullである。両結果のViewerはPC/390px、逆方向replay、page errorと外部requestなしを確認した。

IEEE 802.1AB本文との最終レビューで、mandatory-only shutdown/optional capability省略更新と、RX assembly counterの保存因果検査に不足を検出した。修正後のin-range counter・byte+CRCのrehash改変、実wire撤回の回帰gateは後述の最終実行で成功した。以前の成功件数をこの修正後の成功に読み替えない。

CBSの追加gateは各実segment境界の解析的credit、suspend中backlog、negative creditのままのresume、新規frameのadmission、追加header/mCRC/IPGの帯域会計を対象にする。TASは実advance、gate/HOLD同時刻、inactive時、短いtail、clock変更と保存を対象にする。これらの静的ideal-clock範囲は後述の最終実行と独立レビューで確認した。clock変更は別の結合項目に残る。

## 最終の履歴credit・HOLD検証

mandatory-only LLDP撤回、capability省略更新、実fragmentからのRX組立再計算は修正を完了した。各CBS outputの履歴は有限なtyped cause/実wire proofとして保持し、復元時にzero-creditの新しいTSN状態からqueue・実segment・credit・residual・wakeを再計算する。実packet pruning後もproofは保持する。上限はevents 100000、proof octets 67108864であり、超過は原子的に拒否する。保存creditを正本とする任意のcheckpoint anchorは設けない。

HOLDはpActive喪失で自動解除しない。実行例では897224000psにHOLD、1000576000psにLLDP撤回、1199904000psのinactive RELEASEは無効、2001824000psに実V/Rで再検証、3199904000psのactive RELEASEで同じpMACを再開した。expressはHOLD中にも送信できる。

修正後のcoreは19件（8か所のallocation失敗を含む）、製品経路は24件に成功した。9種類の履歴改変を拒否し、HOLD周辺16停止点とlink-reset周辺3停止点で保存継続が一致した。独立レビュアーも凍結した製品test binaryを再実行して24件すべて成功した。最新の実HOLD Viewerは1440px/390pxでinactive HOLD、再検証後HOLD、実RELEASEと逆方向replay、page error/外部requestなしを確認した。

GCL置換とlocal-clock CBS/TASの結合は別の未完了項目として保持する。選択した静的ideal-clock範囲の成功を、その結合成功へ読み替えない。

[最終独立レビュー](../results/remaining-20261021/mac-merge-causal-final-review.md)を参照する。
