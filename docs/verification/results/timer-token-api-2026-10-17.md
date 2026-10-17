# Timer通知API修正・検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`docs/functional-behavior-reference @ c67e153`
実施日：`2026-10-17`（日本時間）
文書ID：`verification-timer-token-api`
文書状態：作業差分の実施記録。公開タグへの収録前。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-17` | Timer発火tokenの公開通知APIを追加し、公開APIを使う統合試験の入力・期待値・結果を記録 |

## 修正した不一致

[実行仕様の通知契約](../../specs/実行詳細機能仕様書.md#delivery)は、Timer予約時のtokenを発火時に同じモデルへ通知することを求める。基準コミットではEngine内にtokenがあるものの、公開EnvelopeにもContextにも取得口がなく、外部モデルから参照できなかった。

`Context::current_timer_token()`を追加した。Timerの`on_event`では予約時に返したtokenと等しい値への読取参照を返し、初期化・ポート配送・調停では`None`を返す。保持や可変Context操作の前には`cloned()`で値をコピーできる。Envelopeの形、tokenの不透明性、予約・取消し・イベント順序の規則は維持する。

詳細契約は[インタフェース仕様](../../specs/インタフェース詳細機能仕様書.md#22-context操作)、所有権は[実行設計](../../design/実行詳細設計書.md#dispatch)、検証条件は[DIR-TEST-0009](../cases/共通実行検証仕様書.md#dir-test-0009)を参照する。取消しの確認はDIR-TEST-0008の補足も兼ねる。

## 入力・期待値・実測

試験は`crates/dir-simulator/tests/registry.rs`の`current_timer_token_identifies_reservations_and_is_scoped_to_timer_callbacks`。製品内部の非公開fieldを参照せず、登録した二つのモデルを公開API経由で実行する。

| 入力・操作 | 仕様から求めた期待結果 | 実測結果 |
| --- | --- | --- |
| 各モデルでschedule_at(3)、schedule_after(1)、schedule_at(1)を予約。schema・payloadは全て同じ | 1psの二件、3psの一件の順。発火tokenは対応する予約の返却値と等しく、1ps同士は別token | 一致 |
| 各モデルで2psの別Timerを予約し、同じ初期化callbackで二回取消す | 初回true、二回目false。2psに通知されない | 一致 |
| 発火中tokenをcloneし、取消し操作へ渡す | false。次のcallbackで前回発火tokenを取り消してもfalse | 一致 |
| 初期化中、ポート受信、調停callbackで取得する | None。Timerのon_eventだけSome | 一致 |
| Timer callbackで調停を要求し、再びtokenを取得する | そのcallbackのtokenを保持する | 一致 |
| 1psに同じschema・payloadのポート通知を一件配送。資源調停は0・1・3psに各モデル一回 | Timer六件＋ポート一件＋調停六件＝13コミット、取消済みTimerは件数外 | committed_events=13、pending_events=0、events_exhausted、診断0件 |

## 実施した検証

修正前に同じ統合試験をコンパイルし、公開メソッドが存在しないためE0599で失敗することを確認した。修正後はRegistry統合試験38件すべてが合格した。失敗を消さずに、修正前と修正後のログを別々に保存する。

| 検証 | 結果 |
| --- | --- |
| `cargo test --locked --workspace` | 778件合格、失敗0、既定ignore 19件は未実施。Registry38件は778件の内数 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 合格、警告なし |
| `cargo fmt --all -- --check` | 合格 |
| 文書・参照 | トレーサビリティ厳格検査、生成物一致、MkDocs strict、ガイド2689リンク、変更した仕様・設計・検証文書256リンクが合格 |
| 画像内蔵HTML | PC・390px幅で11項目合格。内部リンク106件と画像13要素が解決、横はみ出し・browser errorなし |

通常のRust検証ゲートと文書検証の最終結果は[機械可読記録](timer-token-api/verification.json)を正とする。基準コミット、適用差分、対象ソースのhash、toolchain、実行コマンド・終了コード・ログhashを記録する。基準コミットだけを修正後コードの識別に使わない。

## 確認範囲

汎用登録モデルのTimer通知に対する修正と回帰検証である。組込みprofile全体の規格適合、性能、将来のPause/Resumeやcheckpointの実装完了を示す記録ではない。既存の受入記録にある未確認条件を、この試験の合格だけで完了へ変更しない。
