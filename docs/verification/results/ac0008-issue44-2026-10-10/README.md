# Issue #44 診断reasonの限定修正とWSL実測

文書バージョン：`1.1.0`
文書ID：`ac0008-issue44-reason-verification`
状態：Issue #44の限定修正は回帰合格。AC0008全体は未充足。

### 更新履歴

| 文書版 | 日付 | 内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-10 | import／extendsのreason修正、通常syntax_errorの回帰、実callback測定とWSL証拠を追加 |

[Issue #44](https://github.com/hideki-ozu/DIR-Simulator/issues/44)に従い、未対応import／extendsのreasonを`unsupported_syntax`へ修正した。Draft [PR #43](https://github.com/hideki-ozu/DIR-Simulator/pull/43)を更新する。baseは引き続き[PR #42](https://github.com/hideki-ozu/DIR-Simulator/pull/42)のbranch `codex/ac0008-review-evidence-2026-10-10`、SHA `bfe0ebedaef7ed814d06d2f90ca0a7dbe453c54a`。依存順はPR #40 → #42 → #43である。

修正前PR headは`649dd8ac4fbb4b4abaa0224fee852d2ec467d7e0`。今回実測したsource／test commitは **`9adf18faf1d0791360c4528fe1445b2560e395f8`**。製品変更は`src/input/ned.rs`と`tests/ac0008_ned_rejections.rs`の2ファイル。証拠公開commitはこのsourceを祖先として保持し、製品sourceを追加変更しない。

## 分類と位置の境界

トップレベルの有効なdotted import prefixの`*`を、既存lexerの失敗位置で`unsupported_syntax`にする。wildcardのprimaryは従来どおり`*`。失敗後のsuffixを新たに字句解析して理由や優先順位を変えない。文字列／コメント中やnested／不正なprefixの`*`はこの分類へ漏れない。

parserではtokenを消費しないname-clause確認で、import名列の後に`;`、extends名列の後に`{`がある場合だけ既存診断のreasonを変更する。bare import、import `;`、不完全なdotted name、bare extends、extends `{`等は`syntax_error`を維持する。一般的な字句／構文失敗を作る共通helperも`syntax_error`のまま。

文法対応・仕様緩和・primary／失敗順変更は行わない。後段の不正字句がlexerで先に見つかった場合は従来どおり、その不正字句の`syntax_error`が先行する。E-0001、prepare段階、source、actual／expected、本文と行列・範囲の整合を公開prepare経路でassertする。

## WSLでの実行結果

環境はWSL2上のUbuntu 24.04.4 LTS、x86_64、kernel `6.18.33.2-microsoft-standard-WSL2`。固定Rust／Cargo 1.85.0、host `x86_64-unknown-linux-gnu`、既定debug設定。正確なargv・flags・UTC時刻・exit・named結果は[run-result.json](run-result.json)、toolchainとOSは[environment.json](environment.json)に保存した。

| source 9adf18fで実行したgate | 結果 |
| --- | --- |
| `ac0008_ned_rejections`／`ac0008_attributes`／`ac0008_paths` | 32 passed（24＋4＋4） |
| `input::` library回帰 | 45 passed |
| `diagnostics`／`registry_diagnostics` integration回帰 | 23 passed（15＋8） |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | exit 0 |

対象24試験はimport wildcard／explicit import／extendsの3件、unknown propertyの既存分類1件、通常または本修正対象外の拒否20件を確認する。不正token、括弧欠落、不正string、文字列／コメント内のimport、import／extendsが同じfileにある後段字句不正、nested／不正prefix、上記不完全句を含む。

3件の対象とunknown propertyでは、登録モデルのinitializeが実eventをscheduleし、正常入力のcommitted event=1と実on_event counter=1を確認する。その後counterをresetし、変更入力の`prepare_with_registry`拒否時にcounter=0をassertする。単なる固定値のログではない。plain prepareと登録経路のcode／stage／source／actual／expected／primary／行列を確認し、本文整合もassertした。[runtime-bindings.json](runtime-bindings.json)に実測counterとcase IDを紐付けた。

## 過去証拠の保持と残件

[旧公開packet](../ac0008-vscode-execution-2026-10-10/README.md)の166派生証拠（過去の成功・失敗log／JSON）はbyteを変更していない。[保持ハッシュ](historical-evidence-preserved.json)で照合する。旧packetの入口文書・検証script・checksumだけを、履歴の説明と保存commit照合のため更新した。旧3件の`syntax_error`実測を新結果へ書き換えず、今回のログを追加した。

Issue #44のreason不一致はこの限定範囲で解消した。wildcardの`*` primaryはIssueの方針どおり保持し、不具合として別位置へ移していない。全atomic合格、元Controller行の全修飾条件、430 predicate／318 subcaseの完全fixture／assertion対応は主張しない。完全static link人口、作者出自、各採用条件／50採用承認、UMD／Snowball／CC0原文等のAC0008残件も維持する。

既存main `0b7b23d8e23fcb1a1491cd13cd42aa5b96a9ab1c`と元worktreeは保持した。マージ・Issue close・タグ・Release公開・deploy、権限・認証・設定変更は行わない。完了済み固定artifactの3705member／6manifest／42crate全件監査・再取得・Release再構築は実施しない。GitHubの未起動CIをローカル実測の成功へ置き換えない。

## 公開証拠の照合

個人用path、hostname、email、author metadata、secret fieldを除去した。[evidence-manifest.json](evidence-manifest.json)でローカル原本hashと公開byte hashを区別する。run-resultの`stdout_sha256`／`stderr_sha256`は公開版、`original_*_sha256`は原本に対応する。[SHA256SUMS](SHA256SUMS)は公開packet全体を照合する。[review-and-packaging.json](review-and-packaging.json)に文書gateと独立レビューを記録する。

```bash
python3 docs/verification/results/ac0008-issue44-2026-10-10/verify_evidence.py
```

この照合は保存commit・公開証拠の整合確認であり、製品試験や全件監査の再実行ではない。
