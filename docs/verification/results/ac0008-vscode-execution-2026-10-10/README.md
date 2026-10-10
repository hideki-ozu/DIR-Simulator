# AC0008 引渡し実行・属性修正の公開証拠

文書バージョン：`1.1.0`
文書ID：`ac0008-vscode-execution-public`
状態：Draft PR向けの限定修正と検証記録。AC0008は未充足。

### 更新履歴

| 文書版 | 日付 | 内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-10 | WSLでの限定修正・実測結果を、個人情報を除外した公開証拠として初回保存 |

## 修正と依存関係

[PR #42](https://github.com/hideki-ozu/DIR-Simulator/pull/42)の[引渡し手順](../ac0008-review-2026-10-10/vscode-handoff.md)を実行し、[Issue #41](https://github.com/hideki-ozu/DIR-Simulator/issues/41)のNED属性保存を修正した。Declaration／Parameterが属性名・Unicode／escape解釈後の値・型付き所有者・元SourceSpanを保存し、不変accessorで公開する。同型の複数instanceが同じ宣言・属性を参照する。既存class／unit、schema照合、実行値解決を維持する。重複属性と非数値unitの診断は属性開始tokenを基準にし、本文位置・actual・spanの不整合も修正した。

このPRのbaseはPR #42のbranch `codex/ac0008-review-evidence-2026-10-10`。PR #42はPR #40のWSL方針branchに依存する。mainへの直接マージを前提としない。既存mainと元の作業領域は保持し、独立worktree／branch `codex/ac0008-attributes-evidence-2026-10-10`で公開記録だけを追加する。

| 区分 | 正確なcommit |
| --- | --- |
| 保持したmain／完了済み固定artifactのsource | `0b7b23d8e23fcb1a1491cd13cd42aa5b96a9ab1c` |
| PR #40のbase方針 | `36eeaa4fe1bf77d82d90b42dd8cc057861d46df5` |
| PR #42から追加された元テストsource | `6cd33fbf3b1cb09f61d628974559c560c519b47c` |
| PR #42 head／修正前の実行baseline | `bfe0ebedaef7ed814d06d2f90ca0a7dbe453c54a` |
| 属性保存の最初の修正 | `ffe9c5ea75c93e6fe9ed7710c4f601b7ec0f7306` |
| 最終修正／実際にcompile・testしたsourceとtest | `ddfa7bd916212a0b9fedb6adffa9bdb5e83c8401` |

公開記録のcommitは上記ddfa7bdを祖先として保持する。公開時に製品試験を再実行したとはしない。[source-inventory.json](results/source-inventory.json)と公開検証scriptで、公開branch上の6変更ファイルが試験時sourceと同一であることを確認する。

## 実行環境と結果

実行環境はWSL2上のUbuntu 24.04.4 LTS、x86_64、kernel `6.18.33.2-microsoft-standard-WSL2`。Rust／Cargoは固定1.85.0、host／targetは`x86_64-unknown-linux-gnu`。debug／既定featureの記録で、flags・時刻・argv・exit・各named testは[統合検証記録](results/integrated-checks.json)と[toolchain記録](environment/build-link-toolchain.json)を参照する。

| 最終ddfa7bdで実行した確認 | 結果 |
| --- | --- |
| 3 focused targetの`cargo test --locked ... --no-run --message-format=json -vv` | exit 0 |
| `cargo fmt --all -- --check` | exit 0 |
| `ac0008_paths`／`ac0008_ned_rejections`／`ac0008_attributes` | 15 passed（4＋7＋4） |
| `cargo test --locked -p dir-simulator --lib input:: -- --test-threads=1` | 45 passed |
| `cargo test --locked -p dir-simulator --test diagnostics -- --test-threads=1` | 15 passed |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | exit 0 |

修正前bfe0ebeはcompileと10 focused testsが成功し、fmtは2テストファイルの整形差分でexit 1だった。[baseline結果](results/baseline-tests.json)と失敗ログを保持する。属性値が失われる問題と診断位置の不整合は失敗で再現した。初回ffe9c5eのwildcard import位置assertion失敗も保持し、lexerが先に拒否する`*`を実測どおりcharacterizeした。wildcardなしのimport拒否も別variantで確認する。

matrix／atomicの生成鮮度とpacket unittest 9件はbfe0ebeでの静的資料の検査である。[静的結果](results/static-baseline-checks.json)を最終sourceの全atomic製品合格へ置き換えない。

## 未解決項目

import／extendsは拒否されるが、仕様のreason `unsupported_syntax`に対して実測reasonは`syntax_error`。wildcard importのprimary tokenはlexerの`*`である。今回の試験は実際の診断をcharacterizeするもので、この不一致を仕様適合と扱わない。

[runtime-case-bindings.json](results/runtime-case-bindings.json)に7属性plan、11個の部分atomic対応、拒否6 predicateの7 variantを最終source・実行log・assertionへ対応付けた。helperを単独で実行済みのtestと扱わない。callback回数、元Controller行の全条件、430 predicate／318 subcaseの完全fixture／assertion対応は未検証。元の属性fixtureのdefault後propertyは規定文法に反するため拒否を維持し、回帰内で合法順へ明示的に並べ替えた。仕様緩和、Classical CANの汎用factory移行は行っていない。

AC0008は未充足。完全static link人口、作者出自申告、個別採用条件と50採用承認、UMD／Snowball／CC0原文等が残る。独立レビューに未解決のコード／証拠指摘はないが、これらの受入未完了を解消したとはしない。

## build／linkと旧監査の境界

[現在buildの要約](results/build-link-current-summary.json)と[詳細](results/build-link-current.json)にcompiler artifact、実argv／feature／flags、fingerprint、ELFとsystem libraryを記録した。現在debug binaryのSHA-256は`0929fd31d168057caa520f6de3c097953aae9bbdb09aa03cfd06fc1689ad15a7`。Linux解決graphはroot込み31node／外部30packageで、42compiler artifact record／29package IDや全target台帳の42crateとは区別する。キャッシュ利用により全依存の元rustc argvは揃わず、完全static object／retained section／linker mapは未確定。

外部system libraryは`libgcc_s.so.1`、`libc.so.6`、`ld-linux-x86-64.so.2`。Rust標準library／compiler runtime、17Python wheelと生成用asset、Playwright／browserを配布人口と分け、hashと版を保存した。現在環境のhashから過去の全実行が同一byteだったとは推定しない。

[旧ac0008-status](results/prior-audit/ac0008-status.json)と[fresh-candidate-reverification](results/prior-audit/fresh-candidate-reverification.json)は0b7b23dの完了済み固定artifact監査に属する。ローカル監査では原本とbyte同一のコピーを照合済みだが、この公開packetでは個人用パス等を除去した派生版である。元artifactの3705member／6manifest／42crateの全件監査・再取得・Release再構築は再実行していない。既存569／29集計は個別atomic合格へ転用しない。

## 公開用証拠と検証

このpacketはローカル証拠の選択した派生版である。home／Windows user path、hostname、メール、個人環境PATH、author metadata、secret fieldを除去またはplaceholder化した。API context、private script、実行ファイル、cache、仮想環境は公開しない。placeholderは実行可能pathではなく、相対log参照とsource commitを維持するための記号である。

[publication-manifest.json](publication-manifest.json)は原本のSHA／bytesと公開版のSHA／bytesを別々に記録する。証拠JSON中の既存log／file hashは**除去前の原本hash**として保持し、公開版に一致すると主張しない。公開byteは[SHA256SUMS](SHA256SUMS)で別に照合する。原本への外部参照、prototype対応表、古い失敗記録はそれぞれ当時のscopeを保つ。最終対応表はruntime-case-bindings.jsonを正とする。handoff-result.jsonの操作flagsは最初のローカル実行時の状態で、今回のpush／Draft PR作成状態ではない。

```bash
python3 docs/verification/results/ac0008-vscode-execution-2026-10-10/verify_evidence.py
```

scriptは公開file hash、元log hashとの対応、実測のnamed結果数、source同一性、reason不一致とpartial statusを確認する。製品試験や完了済み監査の再実行ではない。公開作業の文書gate／privacy確認は[publication-checks.json](publication-checks.json)に記録する。

PR #42のstacked baseはmain限定の既存CI対象外で、取得時check-runは0件だった。新Draft PRについても、remoteで実際に起動していないCIを合格と記載しない。workflow変更／dispatch、権限・認証・設定変更、merge／tag／Release公開／deployは行わない。
