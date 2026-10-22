# 選択PHY最終レビューの残件

文書ID：`verification-ieee-phy-final-review-open`
文書状態：未commit・未公開。2026-10-22のsource reviewに基づく実装・検証継続記録。

この文書は初回レビュー時点の履歴である。後続の修正・製品gate・独立レビューは[回復とクロック境界の記録](phy-final-review-closure.md)を参照する。全体判定は実装台帳で管理する。

P01〜P09を一括して完了にしない。実sample、codec/FEC、channel、受信fanoutと各MAC/MDI境界は実装されているが、次の製品経路を修正・追試する。

| 対象 | 見つかった残件 | 必要な実装・証拠 |
| --- | --- | --- |
| P01/P03 | 初回訓練のmaxwait・decode失敗後にFaultから進まず、要求のreleaseも将来のReadyへ依存する | 初回quiet/timeout後の再獲得・再訓練・新規配送と、成功startupが来なくても失敗要求を退役させる経路 |
| P02 | Data-phase outageの成功だけではAN再起動を確認できない | 実Clause98の再起動・中断と役割解決を通す製品gate |
| P09 | PLCA Dataのpending-delay・collision後の回復状態、signal_error消費とinvalid-BEACON timeoutが未接続 | 実MAC/carrier/jamとData状態遷移、正確なpending timer、保存継続 |
| P09 | burstの継続判断が送信開始前のqueue内容に依存する | 送信中又はburst待機中に来た次要求を実timer/countで扱い、timeout/abortでCOMMITを解除する |
| P06 | project数値モデルの3送信者試験と、named PHYの証拠を区別する | named PHYの3送信者とslot境界・late faultの該当契約を確認する |

P04の実coding/FECと独立parity、P05の宣言済み数値精度、P07の3端点signal repeater、P08のT1S codec/fanoutは限定範囲のsource reviewを通過した。最終sourceでの回帰試験、証拠対応と台帳更新は継続する。22symbolを超える全破損の検出や、hardware電気適合を主張しない。

実MDI outageでは影響payloadを破棄し、再訓練後のReadyでMAC claimをreleaseする経路がある。約3.8msという保持時間だけを不具合とは判定しない。初回の永久Faultで将来Readyが来ない場合の退役は別の残件である。raw fractional admissionとgPTP/serializerの結合は独立した最終gateで扱う。

この記録はsource reviewであり、ここに挙げた残件の製品試験合格ではない。完了時は失敗証拠を保持したまま、修正後の実行・source hash・独立レビューへ追記する。
