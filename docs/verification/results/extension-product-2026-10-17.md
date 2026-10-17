# 後続拡張の製品検証記録（2026-10-17）

文書バージョン：`1.1.0`
対象GitHubバージョン：`docs/functional-behavior-reference @ c67e153`
検証対象：上記commitを基準とした検証実施時の未コミット差分。build source SHA-256は`51f85805d5724c6ab4ea58553912ef343f889c5b9cca4c17e53a426e155c5008`。
収録リリース：`v1.1.6`（以下の測定・binaryの同定情報は実施時点を維持）
文書ID：`verification-extension-product-20261017`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-17` | 限定scopeの製品検証を記録し、測定source・binary・結果を保持してv1.1.6へ収録 |

文書状態：採用scopeの実装・結合検証を行った時点の記録。実施当時は未コミット・未公開。

[正式採用仕様](../../specs/後続拡張採用詳細機能仕様書.md)の10分野を対象に、実際のRust API／CLI／exported Viewer HTMLで検証する。[機械可読記録](extension-product-2026-10-17.json)にコマンドと結果を保存する。

Gateway＋TSN、抽象冗長配送、抽象preemption、IGMPv3、MLDv2、能動RSTP、能動MVRP、協調RunSession、Metric V2、同一buildのgeneric backend checkpointを実装した。IEEEの全機能・全guard組合せの規格適合や大規模性能を表すものではない。network checkpointは初期scopeの対象外で、明示拒否する。

既存の1322件は設計vectorであり、製品コードで期待値を比較したケースと区別する。 製品コードを呼ぶkernel単体、prepared stateを用いたEngine結合、通常公開API／CLI、実HTMLの画面検査は別の検証層として記録する。`kernel_only`やsynthetic inputのケースは通常CLI入力の受理や規格PDUの適合を表さない。設計checkerの成功だけを製品成功に数えない。テストの追加で不一致が見つかった場合は元の失敗を保存し、仕様に従ってソースを修正して再検証する。

実施済みの個別検証：control／Metric V2／checkpoint、registered Engine、checkpoint bundle、dynamic policy、wire codec／状態機械、通常network API、公開出力を検査した。Viewer modelの12件には、独立に実行した途中終了結果と全体結果の巻戻しを比較する3件を含む。各suiteの最終件数・コマンドは機械可読記録で確定する。

## 発見した不一致と修正

- Learning受信後、処理中にForwardingへ変化すると中継される問題：受信時の中継禁止判定を保持し、MAC学習後は`stp_learning`で終了する。元の再現入力・失敗出力を保存し、再buildしたCLIと回帰試験で確認した。
- Viewerの巻戻し：将来の最終fragment予定・fragment/gap参照を先取りしない。`observed_ps`からprocessing受信を表示し、配送機会は参照元frameの生成時刻から表示する。実際の全体／途中終了結果から選択した変更なしのrecordをfixtureに保持し、入力hashと選択条件を記録した。
- 旧TSN Viewerの互換性：非wire profileにはwire用の空tableを追加せず、従来のsnapshotキー集合を保つ。元の失敗を残し、新しいCLI出力を使う111件の既存Viewer試験で確認した。
- 協調制御・checkpointの独立レビュー：非公開restore失敗時のcleanup、pause世代overflow、Completed後Stopの再演、再演I/Oの論理destination衝突、Ready保存時のgauge状態、失敗時の計測Hを修正し、元の再現5件を変更せず再実行した。

追加の結合修正では、MVRPの管理mode reset時に旧join／periodic timerが残る問題を修正した。旧timerを無効化してからBeginと需要の再構成を行う。同callbackで複数portの登録sourceが交代した際、連続して必要な他portへ一時的なLv／Joinが出る失敗も再現し、全portのTreeBatchからMAPを一回だけ実行するよう修正した。paired RSTPとMVRPへの同一Link通知による`tcDetected`の上書きも解消した。

公開transitionとmetadataはu64を十進文字列で保持し、machine別の有限な変数・guard catalogで検証する。domainの13fieldとhost追加の`effect_seq`を分けて検証し、欠落・数値・非正規表現のsequenceを公開前に拒否する。PRSの同一entryに付随するport観測行は状態機械の実行回数へ加算しない。MVRPのBegin resetとMAPによる需要変化は、実際の変更前後を既存transitionへ記録する。Newのfalse→true、撤回のtrue→falseを実Engineで検査し、Applicant状態と送信要求が不変の需要だけの変化も検査した。

制御APIの`RunSession::snapshot().common.end_ps`に予定Tが残る不一致も再現し、Ready／Running／Paused／Stoppedの成功Hと正常CompletedのTを統一した。Pause時の公開snapshotとexportの通信占有量が一致し、復元時にはjournal終端とHの不一致をfactory実行前に拒否する。同期legacy runの観測終端は変更していない。正常完了後にcloseする場合も、未処理要求の`SessionClosed` ackが保存済みTと一致することを追加検証した。制御APIの全45件が成功した。

## 設計vectorの前提修正

RSTP `P-RSTP-T01-V248/V249/V250`は、未指定boolean=falseのため`sendRSTP=false`となり、上位guardのALTERNATE_PORT self-loopが先行していた。意図したrapid guardの入力として`before.sendRSTP=true`を明示した。IEEE 802.1Q-2022 13.28.9／Figure 13-26を照合し、元のfalse条件のself-loopも別に実行する。期待結果は変更していない。

`P-CORE-T01-V202`は、同bridgeのadapterがそれぞれ全接続portを所有する前提を満たさず、重複ownerの検査より先に拒否されていた。重複する両adapterへ同じ完全port集合を指定し、重複ownerという期待結果は維持した。元のfixtureと失敗、変更前後SHA-256、根拠はローカル証跡に保持する。前提修正だけでは製品合格に数えず、修正後の入力を製品で実行した結果を別に記録する。

`P-CHECKPOINT-T02-V02`は、余分なfileを`inputs/extra.bin`へ変更し、許可されたbundle namespace内でmanifest未掲載のfileがある場合を検査する。元のroot直下`extra.bin`はnamespace違反として先に拒否されるため、元入力の拒否も追加のhardening試験として保持する。`valid=false/reason=unlisted_file`の期待は維持した。

prepare負例の`empty/odd_hex/non_hex`などはfixture内の条件分類であり、公開`Diagnostic.reason`のenum名を新設したものではない。対応する実際の入力、返されたreason/message/target、分類の根拠を証跡に残す。

## 画像と実行範囲

[現行利用ガイド](../../guide/reference/後続10分野の現行実装と利用境界.md)にPause、分割送信・再開待ち、wire port状態の実画面3枚を追加した。[画像証跡JSON](../../guide/assets/reference/extensions/screenshot-evidence.json)に入力・exporter・HTML・画像hashとviewport条件を残す。既存の固定commitの画像10枚は撮影時の証跡として保持する。

通常runの追加profileサンプル5件とschema3 Paused／restoredの計7HTMLについて、1440×1000および390×844表示、前後再生・slider往復、browser error・外部request・横overflowの不存在を確認した。これらは実際のRust exportである。binaryと入力hashがその検証範囲を定め、後のソース修正を自動的に検証済みへ含めない。最終build後の再実行はJSONに別記する。

## 最終結果

| 検証 | 結果 |
| --- | --- |
| Rust workspace（通常実行） | 1038成功、失敗0。native隔離19件は既定ではignore |
| native隔離試験の明示実行 | 19成功、失敗・ignore 0。既定のnamespace保護を使用 |
| `cargo fmt --all -- --check` / Clippy全target `-D warnings` | 成功／警告0 |
| Node全体 | 228成功、失敗0、環境条件によるskip 1件。理由はJSONに明記 |
| 設計vectorの製品検証対応 | 1320成功、設計限定2件、未実行0、失敗0 |
| traceability厳密検査 | 258要件・71機能・318node、不完全・構造エラー0 |
| ガイドstrict buildとリンク | 35 HTML・2826箇所、エラー0 |
| 実CLI・checkpoint・Viewer | 通常5サンプルとschema3 Paused／restored、7 HTMLを両viewportで確認 |

[全vectorの対応表と観測値](extension-product-2026-10-17/vector-coverage.json)はfixture IDを保持する。wire1207件とnetwork31件には期待値と実測値の比較を残し、bridge順序1件は実際のEngine/Arena成功公開を観測する。残る共通・制御・bridgeの81件は実行済みassertionへの対応表で、そのうち70件は個別raw値を保存していない。実行数と証拠の粒度を区別する。設計限定2件は規範本文や設計表が欠落した場合の仮定上のreadiness gateであり、現在のruntime入力ではない。

別processへのcheckpoint復元後と連続実行の`simulation`全体が一致した。比較には同一の凍結したexample executableを使用した。実行時のbinary／build source／入力hash、全コマンド、ログhash、任意skip、fixture前提修正は[機械可読記録](extension-product-2026-10-17.json)から追跡できる。network checkpointは引き続き明示Unsupportedである。
