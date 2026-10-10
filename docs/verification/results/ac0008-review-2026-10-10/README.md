# AC0008 残余証拠・レビュー提案

文書バージョン：`1.1.0`  
文書ID：`ac0008-review-evidence`  
対象：固定製品 `0b7b23d8e23fcb1a1491cd13cd42aa5b96a9ab1c`  
状態：未承認・未完了。合格証明ではない。

### 更新履歴

| 文書版 | 日付 | push単位の内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-10 | WSL方針PR #40を基礎に、独立した残余レビュー資料・未実行テストを初回保存 |

## 固定証拠と実行境界

製品artifact SHA-256 `d6ff7c7df26aed238d725fc661a4f9ebbe4469acce30342f30619ece84ca201e`、3705 member、3593 source、64 guide、6 manifest、台帳1.1.1の機械的照合済みという既存証拠を維持する。この作業では全member監査・42crate取得・Release再構築を再実行しない。

既存569 passed / 29 suitesは提供されたWSL集計の参照値であり、各条項の合格証明に転用しない。正確な `ac0008-status.json` と `fresh-candidate-reverification.json` はLibrary検索で取得できず、完全な実行ログ・build/link情報も不足する。現PCはWindows、WSL照会は `Wsl/E_ACCESSDENIED`、Rust toolchain未発見。Linux cloud実行をWSL Ubuntu24.04の代替にしない。追加Rustテストのcompile/fmt/testは全て `not_run`。新テストcommitは `6cd33fbf3b1cb09f61d628974559c560c519b47c`。本資料の生成・整合検査はWindows Python3.11であり製品試験と区別する。

Windows既存build exit101/17errors/0runsは実行不能の履歴であり、決定性不一致の結果ではない。PR #40のWSL限定方針を保持し、Windows native比較は再開しない。

## Classical CANの実経路と0085の未承認案

CLI `main.rs` → `run_config` → `prepare_with_registry_and_source` は既定RegistryのBuiltinAdapterを選び、Classical CANは汎用factory/callback構成ではなく既存CAN Engine経路へ接続する。素の `input::prepare` → `prepare_with_source` は登録なしのPreparedを返し、公開 `run` が従来CAN経路を実行する。汎用Registryのモデルfactory/コールバック経路と区別する。根拠は固定commitの `src/run.rs`、`src/registry/prepare.rs`、`src/input.rs`、`src/registered.rs`、`src/engine.rs`。

`ac0008_paths.rs` は既存competition fixtureを用い、CLI/run_config/prepare+run/prepare_with_registry+runの4入口を比較する。simulation JSONの観測可能部分を基準とし、実行メタデータを除外する。解析的SOF/EOFも別assertionで確認する。結果は未実行であり、同値を実測済みとはしない。

DIR-TEST-0085の「Controller factory」「Bus channel factory」「イベントcallback」は実装と文字通り一致しない。受入れ文言案：『既定RegistryのBuiltinAdapterがClassical CAN設定を選択し、従来CAN Engineへ引き渡す。汎用登録モデルではfactory/channel factory/callbackを別途検証する』。これは承認前の提案であり仕様を変更しない。CANを汎用経路へ移行しない。

## NED条項・診断・属性の不足

`ned-rule-matrix.json` は37主要群（9+11+10+7）と周辺規範文を395条項に展開する。`ned-subcase-matrix.json` は0060～0069を201操作候補に展開する。元行・修飾条件を保持し、公開OMNeT6.4章節・固定source位置・名前付きtest候補を付ける。分割は句点/読点・slashによる機械的索引であり、独立オラクルが揃った完全な合格マトリクスではない。候補testの存在からatomic passを推定しない。公開節対応はレビュー候補で、DIR固有制約と上流互換を区別する。複合条件の追加分割・具体的positive/negative入力と診断オラクル・WSLログ紐付けは未完成。

6個の新しい拒否テストはimport/extends/vector/非修飾型/inout/allowunconnectedのpublic prepare拒否とNED source位置を確認する予定。完全な理由文字列・所有者・各006xオラクルの代替にはしない。

Declaration/Parameterにはdisplay/descriptionの値・所有者・span保存フィールドがなく、property処理はclass/unitだけを保持する。[Issue #41](https://github.com/hideki-ozu/DIR-Simulator/issues/41)に確認箇所と限定修正案を保存した。仕様を弱めず、宣言/parameter属性をowner+value+SourceSpanとして保存し、read-only accessorと同じ型の複数instanceからの参照を設計する。Unicode/escape/BOM/CRLF・宣言/parameter所有者の回帰オラクルは `attribute-regression-plan.json`。実装修正とその実行はWSL・具体的保持APIの不足により保留する。

## 出自・採用・配布人口

`origin-attestation-packet.json` は固定tracked treeの1067 NED/INI/tests/fixtures/generated/parser項目を列挙する。file追加履歴を確認できた項目と未照会項目を分ける。directory履歴を各file導入の証明にしない。Git authorや自作parserは独立創作の証明ではなく、作者申告は未取得。OMNeT比較ツール撤去commit3件を維持し、コードを復元しない。

`ledger-review-proposals.json` はCargo42+guide8の50既存行を元行・安定key付きで保存する。ORは未選択、各採用承認・実配布member/build/link対応は未確認。MITならcopyright/permission通知を保存、ApacheならLICENSE/適用NOTICE/変更表示等を検討する。MPL1.1は該当covered fileとsource提供条件を検討し、プロジェクト全体の自動判定にしない。個別route・通知・source/archive/binary人口への割当てが承認されるまでAC0008は未完了。

UMDを追加subcomponentとして残す。固定UMD commitのMIT原文とreturnExports wrapperを読み、lunr3fileの帰属コメント・adapted wrapper行を確認した。wrapperは改変され、同一byteや正確な導入revisionを証明していない。原文保存提案は `proposed-umd-MIT.md`。最終guideへの採用/通知は未承認。Snowball0.3のOleg Mazko/Urim/MPL表記は出自未解決で、現代BSDへ置換しない。rustix CC0原文HTTP403は不足として保持し、別経路を含む再取得をしない。

WSLのrustc/Cargo/host/target/features/flags、実build graph、ELF interpreter/dynamic/static依存、system library版、Python wheels、Playwrightの配布/build専用区別とhashが必要。Windows名の依存を名前だけで除外しない。固定artifactの監査済みmemberと実人口を結ぶ証拠が不足する。

## 再生成・レビュー

`python scripts/verification/ac0008_matrix.py`、`python scripts/verification/ac0008_packet.py` でJSONを再生成する。`ac0008_matrix.py --check` と `test_ac0008_packet.py` は索引の再現・一意key・50行/37群・未承認状態を検査する。製品のWSL試験とは別。

独立PRはPR #40 branchをbaseにするため、mainのみを対象とするCIは自動起動対象外になり得る。remote exact headと実際のchecksを確認し、CI未実施を成功と書かない。merge/tag/release/deploy、設定変更、外部レビュー依頼は行わない。Ethernet期限完成稿は停止状態を維持する。
