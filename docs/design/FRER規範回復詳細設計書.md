# FRER規範回復詳細設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`design-frer-vector`
文書状態：v1.2.0収録。本契約の製品gate・独立レビュー済み。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="vector"></a>

```trace
{"id":"design-frer-vector#vector","stage":"design","requirements":["DIR-REQ-0312"],"upstream":["architecture#frer-vector"],"state":"confirmed","pending":[]}
```

既存NetworkState/NetworkDelta内のReplicationStateを拡張する。streamごとに生成器、履歴bitmap、最終受理時刻、timer generation、deadline、reset journalを保持する。受理時刻tとResetMSec=Nに対しdeadlineは`(floor(t/1ms)+N)*1ms`である。共通Controlに所有されたphase0の`RecoveryTimeout`を予約し、受理更新で旧timerを置き換える。予約失敗ではstateとeffectをともに公開しない。

endpointはtyped recovery journalで各受理と実処理完了を結合する。bridgeはContinuationRoundを受理ごとに生成し、SuffixRound cursorで通常queueとtransfer/receptionを通す。legacy once-only suffixとnormativeな複数roundを型で区別し、既存summaryのwinnerを規範回復の制御条件にしない。

checkpointはnormative単独にversion12、MAC Mergeとの組合せにversion13を選択する。実sequence割当、受信wireから復号した入力、受理・拒否・timeoutを再生し、履歴、次sequence、deadline、reset、endpoint配送、suffix roundと物理親子関係を検査する。deadlineと所有Dueを全単射で対応させる。同時刻のcheckpointではFESのdelta/phaseとcommitted cursorも検査し、処理済みtimeoutを後続deltaのControlに戻す改変を拒否する。

この検査は選択した保存形式の意味的整合性を検証する。署名による発行元の真正性や、入力・全履歴を一括で改作する攻撃への認証ではない。出力とViewerは受理件数と実配送件数を分離し、途中状態と時刻を戻した状態にも同じ区別を適用する。
