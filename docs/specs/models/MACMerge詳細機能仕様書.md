# MAC Merge詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-mac-merge`
文書状態：未commit・未公開。選択した静的ideal-clock契約の製品検証・独立レビュー完了。

<a id="merge"></a>

```trace
{"id":"spec-mac-merge#merge","stage":"spec","requirements":["DIR-REQ-0313"],"upstream":["DIR-FUNC-0090"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | Clause99の実packetとshaper結合の契約を追加 |

`ethernet.network.composite.v1`の`mac_merge`はIEEE 802.3-2022 Clause99のMAC Mergeを明示選択する。既存`ethernet.preemption.abstract.v1`は独立した抽象モデルとして保持する。初期の実packet契約は100Mb/sまたは1Gb/s、全二重の対向2port、固定rate、zero MAC/RS delay、store-and-forward RXとし、4種すべてのaddFragSizeを扱う。PHY電気適合や機器全体の遅延認証はこの契約に含めない。

eMAC/pMACは同じ媒体serializerを使う。実LLDP受信から相手のsupport/addFragSizeを学習し、実Verify/Respondと所有timerで有効化する。構成値や予定到着だけからverification成功を作らない。相手のenabled/activeを前提にした循環待ちは作らない。受信したMSAPのTTL=0による撤回、optional capabilityを省略した更新、期限切れ、link epoch変更を反映する。

pMACは実express demandまたはHOLDでのみ、最小mDataと残りMAC長を満たすoctet境界で中断する。SMD、frag_count、継続preamble、mCRC、最終FCSを実byteとして運ぶ。inactiveで始まったframeを途中のverification成立で分割しない。受信側は実byteの順序・counter・CRCから組み立て、完全なMAC/FCSを得てから通常のL2/FRERへ渡す。欠けたmiddle/final fragmentから成功配送を合成しない。

CBSは実frame送信区間だけをsendSlopeで扱い、中断中と追加のpreemption overheadはtransmit=falseとする。選択済みpMACはsuspend中もbacklogを保持し、resume時に新規frameのcredit admissionをやり直さない。TASは元frame全体の保守的fitを保ち、SetAndHoldMAC/SetAndReleaseMACの実advanceを適用する。これらのshaper結合は製品gate完了まで実装中として扱う。

## 最終の履歴credit・HOLD検証

mandatory-only LLDP撤回、capability省略更新、実fragmentからのRX組立再計算は修正を完了した。各CBS outputの履歴は有限なtyped cause/実wire proofとして保持し、復元時にzero-creditの新しいTSN状態からqueue・実segment・credit・residual・wakeを再計算する。実packet pruning後もproofは保持する。上限はevents 100000、proof octets 67108864であり、超過は原子的に拒否する。保存creditを正本とする任意のcheckpoint anchorは設けない。

HOLDはpActive喪失で自動解除しない。実行例では897224000psにHOLD、1000576000psにLLDP撤回、1199904000psのinactive RELEASEは無効、2001824000psに実V/Rで再検証、3199904000psのactive RELEASEで同じpMACを再開した。expressはHOLD中にも送信できる。

修正後のcoreは19件（8か所のallocation失敗を含む）、製品経路は24件に成功した。9種類の履歴改変を拒否し、HOLD周辺16停止点とlink-reset周辺3停止点で保存継続が一致した。独立レビュアーも凍結した製品test binaryを再実行して24件すべて成功した。最新の実HOLD Viewerは1440px/390pxでinactive HOLD、再検証後HOLD、実RELEASEと逆方向replay、page error/外部requestなしを確認した。

GCL置換とlocal-clock CBS/TASの結合は別の未完了項目として保持する。選択した静的ideal-clock範囲の成功を、その結合成功へ読み替えない。

[最終独立レビュー](../../verification/results/remaining-20261021/mac-merge-causal-final-review.md)を参照する。
