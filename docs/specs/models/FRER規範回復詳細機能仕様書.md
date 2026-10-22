# FRER規範回復詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`spec-frer-vector`
文書状態：v1.2.0収録。本契約の製品gate・独立レビュー済み。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="vector"></a>

```trace
{"id":"spec-frer-vector#vector","stage":"spec","requirements":["DIR-REQ-0312"],"upstream":["DIR-FUNC-0089"],"state":"confirmed","pending":[]}
```

streamの`recovery_algorithm="vector_8021cb_2017"`を明示すると、IEEE 802.1CB-2017のsequence生成、非Individual vector recovery、R-TAGを選択する。省略時は既存`project_history_v1`の挙動を保持する。R-TAGというwire名だけで回復algorithmを切り替えない。

履歴長は2〜32767、回復timeoutは正の整数msをpsで指定する。16bit sequenceを符号付きmodulo差として比較し、差が履歴長以上またはその負値以下ならrogueとして拒否する。duplicateとrogueはtimeoutを更新しない。受理したadvance/lateは履歴とtimerを更新する。1000Hzのglobal tickを選択し、timeoutの実イベントは同時刻の受信より先に処理する。timeoutは履歴をTakeAnyへ戻すが、独立した送信sequence生成器を初期化しない。

timeout後は同じsequenceでも再受理され得る。その受理をendpointでの実配送、またはbridgeの新しいsuffix転送へ一対一で結合する。以前のsuffixが完了・喪失・伝搬中でも、別の受理をorigin単位の一回制限で消さない。

実R-TAGは`f1 c1`、予約2octet、network order sequenceの6octetである。予約領域は送信0、受信時は無視する。実wireのFCS検証後にsequenceを復号し、tag除去後は必要なpaddingとFCSを再生成する。外部の最小64byte tagged frameを除去して58byte frameを出力しない。

対象は明示stream、Vector_Alg、IndividualRecovery=false、TakeNoSequence=false、選択したVLAN stackである。Match_Alg、Individual recovery、latent error detection、自動構成、HSR/PRP、全managed objectや機器全体の規格認証を含めない。本文の参照箇所は802.1CB-2017 §7.4.1〜7.4.3.4、§7.8、§10.4.1である。取得した規格PDFはignoredのローカル参照資料とし、公開成果物へ含めない。
