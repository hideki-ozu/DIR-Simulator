# gPTPとWire serializerの実装・検証

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`review-remaining-gptp-wire-20261022`
文書状態：未commit・未公開。記載した限定範囲の独立レビュー記録。

gPTPのordinary / MAC Merge EXPRESS / abstract EXPRESSの3送信ownerを既存NetworkState/FESへ接続した。保持するproducer source履歴、実SOF/EOF/受信captureのjoin、転送indexとepochを照合する。packetの受理前に得られないtimestampや、破棄後のFollowUpを生成しない。全体の最終判定は[実装台帳](../../../implementation/remaining-capabilities.json)と後続の統合検証記録を参照する。

## sourceと製品gate

| 範囲 | 実施記録 |
| --- | --- |
| configured gPTP | 20試験、ordinary/Merge/abstractの実送受信、FES、source改変と別process復元 |
| gPTP通常 | 25試験。時計選択・同期・欠落・再選択とTSNへの実時計反映 |
| public source identity | 実4ownerと11変異。数値のattempt/transfer indexも既存producer証拠へ結び付ける |
| ordered source reset | 新規5試験。3ownerの15固定ABA、6同値/初期Down操作、実Syncによるstale join、2別process保存と12再hash変異 |
| Wire serializer | 実FIFOをClause99/abstract EXPRESSへ投入し、候補固有のsize・TAS・CBS、local clock、PAUSE、epochを照合 |
| link reset | active/suspended DataとWire、実部分byte、実EOF後と受信成立後のcut、23のData reset cut、3別process保存と6強い変異 |
| raw Wire保存 | 実sampleの別process継続と、source/sample/creditを併せて改変・再hashした7変異をfrozen-input prefixで拒否 |
| 実byte独立oracle | `docs/verification/fixtures/wire-serializers/oracle.py`でCRC/mCRC、元byte、IPG、有理時刻丸めとlate PCSを照合 |

LLDPのclassは通常のuntagged port defaultを使用し、actual byte数のgate fitとCBSを検査する。Verify/Respondの既存規格用処理と区別する。標準で禁止された組合せは具体的診断で拒否し、PAUSE送信とproject preemptionの組合せは明示的な`dir.experimental.project_preemption_pause.v1`だけで有効にする。

## 独立レビューと残件の解消

`wire-producer-closure-review.md`はlocal/directional変更hook、数値source index、abstract LinkSet到達、fractional admission、実reset退役とsource履歴を確認した。そこで残ったconfigured Merge + CBSのNative到着時刻と同時刻wakeの試験不足は、[後続レビュー](native-input-final-review.md)で閉じた。到着時刻はEOFに送信PHY・伝搬・受信PHYを加算し、+1psをco-forgeした状態でも拒否する。旧10us wakeは無効、新しい10us wakeが共通Control FESを経て実SOFへ進むことを実Engineで確認した。

これらはselected source/fixtureの実行とsource reviewである。最後に統合したsourceのworkspace、Clippy、CLI、Viewerと文書gateは統合検証記録へ記載し、過去の失敗logを合格として扱わない。raw Qccの範囲は[専用記録](qcc-bootstrap-final.md)を参照する。
