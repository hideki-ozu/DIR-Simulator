# 選択PHYの回復・クロック境界の検証

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`verification-ieee-phy-final-review-closure`
文書状態：未commit・未公開。選択契約の製品gateと独立レビュー記録。

[初回レビュー](phy-final-review-open.md)で見つかったstartup/AN/PLCAの製品経路を修正した。本書はP01〜P09の限定した契約を扱う。最後に統合したsourceの全体合否は実装台帳と統合検証記録へ記載する。

| 範囲 | 実際の製品経路と検証 |
| --- | --- |
| P01/P02 | 初期maxwait・training decode失敗から再取得し、新しい実MACを配送する。実Clause98 page/ACKの中断・再起動、role conflictと終端失敗要求の退役を確認 |
| P03 | 管理down、実MDI outage、PCS/FEC破損とMAC復号失敗を区別し、影響payloadを破棄して実trainingから配送を再開 |
| P04/P05 | 選択codec/FECの独立parityと22symbol訂正境界、宣言したFIR/ADC/noiseと32/64sample精度包絡。全誤りの検出・hardware電気認証へ拡張しない |
| P06/P07 | named T1Sの3送信者・tap別carrier・JAM/BEB・late境界と、3port signal repeaterの実遅延・jam・partition/reconnection |
| P08/P09 | 実受信ADCからの制御取得、sparse/故障重複ID、coordinator喪失復帰、Control/Dataの全選択状態、burst中と送信後の新規到着 |

## actual sampleと時計

HOLDのforced collisionはMAC側だけに32bit JAMを生成する。要求なしbaselineと実MDI TX/RXを照合し、BEACONを中断せず、偽の物理frame/originを生成しない。実物理TRANSMITのcollisionは媒体上JAMとBEBを保持する。pending timerはJAM終了後512bit、WAIT_MACは288bitであり、後者をPCS待ちで再開始しない。

MCD/PMCDは既存Transmitterのindexとclock_ppmから有理位相交差で導く。BEACON/COMMIT/dataの開始を同じclockへ合わせ、群とSILENCEは連続出力する。ゼロppmのPMCD phase120→MCD phase0、実5群と終端zero、および管理disable/re-enableの±10000ppm制御波形を確認した。実payloadの±100ppm gateでは、開始遅延1280sampleに対してFLUSHが1281又は1279sampleになる両方の丸め例を、正しい受信byteで確認した。source/output開始のニブル番号から終端を導き、同じProgramがESDまで継続する。

## COMMIT解除の独立レビューと再現

独立source reviewは、COMMIT commandがMCD間で解除された時に開始済み群を途中で捨てる不具合F1を指摘した。旧sourceを使う新しい実波形試験で、command解除35201の後、sample35216のchip1が期待1000000uVに対して0になる失敗を再現・保存した。

修正は既存queueの残群を排出し、将来の群を生成しない。commandがfalseでも残群中のProgram開始を抑止する。restoreは既存oscillator cutから残長を照合し、過長・位相不一致・zero/不正値を拒否する。実collisionで必要な割込みは維持した。後続Astra reviewはproduction SHA256 `cc6f33a046b74cbea8b7b925044375a13764175b7046562723942bfa8516ae10`を確認し、追加の具体的なP2を指摘しなかった。親側で同hashと後続の合格gateを結び付けた。

| TX ppm | command解除 | 最終群開始 | idle開始 | 保存時の残chip |
| ---: | ---: | ---: | ---: | ---: |
| 0 | 35201 | 35200 | 35360 | 9 |
| -100 | 35206 | 35204 | 35364 | 9 |
| +100 | 35198 | 35197 | 35357 | 9 |

sample単位の独立DME oracleで全10chipとliteral COMMIT code、通常cadence、後続2ニブルに余分な群がないことを検査した。残群中のcheckpointは全効果と最終状態が一致する。Networkの実sample188162で残9chipを保存した別process継続も配送まで一致する。合法な長さ・値のまま極性だけを改変しenvelopeを再hashした状態も、frozen-input prefixの照合で拒否した。

## 実行記録の範囲

- COMMIT修正後：T1S 31 PASS・1 ignored、Network通常4 PASS・2 ignored（5保存cut・16変異）、PHY単体50 PASS。WAIT_MAC 288bitと既存forced/physical JAM gateを含む。
- 修正前のsource18：coordinatorの実13ms復帰1、管理cut/終端AN2、T10 19、medium18、channel15、boundary4がPASS。
- source17の1000BASE-T1実sample全13試験がPASS（642.58s）。完全な97.5ms初期maxwait、AN中断、訓練失敗、raw delimiter/PCS、伝搬の分数境界、outage/retrain、sparse FIRとcheckpointを含む。

後続のCOMMIT修正はPLCAの既存queue/guardとその保存検査に限り、GbT1のstartup/AN/codecとreceiver/attachmentを変更しない。source18と修正後の対象ファイルhash比較を保存した。最終統合版では全通常workspace gateと、実raw gPTPの0/16/17sample境界を別に実行し、compiled source identityを照合する。

詳細な失敗・合格log、波形CSV、正しい保存stateと変異workerはローカルevidenceの`phy-recovery-completion/`、`commit-drain-fix/`、`commit-drain-review.md`にある。参照規格の版・選択optionは[manifest](../../fixtures/ieee-phy/source-manifest.json)を参照する。
