# Ethernet配送期限の実測証跡

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
文書ID：`evidence-ethernet-deadline`  
文書状態：実測・実Viewer確認済み。公開ガイドの表示・検索・Project登録は別途確認

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-10` | 初回push。実測原本のhash確認、全record・指標・入力snapshot、実Viewer画像の根拠を保存 |

## 実行と再確認の範囲

OMEN35Lの通常VSCode実行環境で2026-10-10に取得した原本を基にする。WSL2 Ubuntu 24.04.4 LTS x86_64、Rust/cargo 1.85.0、sourceは`b45515644dfc65c205216d564d5bf7ef00280622`。v1.1.3のCLI実測ではない。binary SHA256は`9996ab4f0a8f0435b39d06e6be0fe7b9afc001ae89b20beffa7bf3027d7b1de4`。

通常入力とZIPを新規空フォルダへ展開した入力で、3条件のvalidate/run/viewを各1回実行し、合計18操作がexit 0。全原本161ファイルとログhash、各結果のmanifest掲載ファイル、input snapshotと入力hashを受領後にも照合した。CLIを再実行していない。[受領再確認](received-verification.json)と[通常結果](deadline-analysis.json)、[ZIP結果](zip-deadline-analysis.json)を参照。

通常・ZIPの`simulation`全体は条件ごと8,025 scalar leavesが除外なしで一致した。[比較記録](prepared-versus-zip-simulation-equality.json)。条件間ではdeadlineとglobal/Endpoint bのmiss・ratioの計5値だけが異なる。[差分記録](condition-differences.json)。1条件につきframe 1、transfer 2、reception 2、metric records 155、summary metrics 373を省略せず保存する。標本のglobal/Endpoint集計を加算しない。

## 元の実行版判定と一行修正

元checkerの`metadata.runtime_version == '1.1.4'`は不適切だった。公開固定sourceのCargo packageは0.1.0で、出力側は`env!("CARGO_PKG_VERSION")`を記録する。Git Release版1.1.4と別である。元checkerを[原本](check_deadline_results.original.py)に保持し、[コピー](check_deadline_results.runtime-adjusted.py)ではそのliteralを`'0.1.0'`に1箇所だけ変更した。[diff](runtime-adjustment.diff)と[版の証跡](compatibility-evidence.json)を保存。source commit、compiler、binary hash、入力・manifest・record・指標の検査は変更していない。

コピーの再確認例（CLI実行を伴わない）:

```bash
python3 docs/verification/results/ethernet-deadline-2026-10-10/check_deadline_results.runtime-adjusted.py \
  --execution-root /absolute/path/to/original/deadline-output \
  --inputs-root examples/guide/ethernet-deadline
```

元execution.jsonや分析JSONには取得時の`analysis_pending`/Viewer未実施状態が残る。原本を後から完了状態に改変していない。後続の[実測まとめ](measurement-summary.json)、[Viewer取得記録](viewer-capture-report.json)、[画像目視](visual-review.json)と併読する。公開コピーのhistorical status自体も保存している。

## 公開コピーと原本を区別する

分析JSONは全model/metric/summaryを保持する。公開用のmetadata・取得記録では個人の絶対パスだけを論理的なplaceholderへ置換した。原本の結果/manifest hashは原本を指し、公開コピーのbyte hashと同一ではない。[input snapshots](input-snapshots.json)のcontent_utf8とsha256は改変していない。全6実行のresults.json、CSV、manifest、CLIログ、Viewer HTMLの原本は受領フォルダに保持し、この派生資料を原本CLI出力と称さない。

[189 build source hashes](build-source-inventory.json)は実行者の原本取得記録に基づく。公開固定sourceのCargo.tomlとoutput/ethernet.rsは別途byte/hash照合した。189ファイル全部をこのPCで公開sourceから再取得して比較した、という主張はしない。[v1.1.3/v1.1.4比較](source-version-comparison.json)では入力4ファイルが同一、flow実装の差は[patch](flow-version-difference.patch)へ保存。deadline比較式はこの差分で変わらないが、全挙動同一やv1.1.3実測を意味しない。

Chromium 151.0.7922.34でshort/equalを共に1,200,000ps、packet:0選択で撮影。1440pxデスクトップと390px詳細の元PNG4枚を加工せず公開assetsへコピーし、全画像を開いて目視した。Viewer 390px確認はガイド本文の390px確認と区別する。今回のガイド本文のMkDocs・リンク・検索・390px・Projectは各検証記録で確定するまで未確認。

## 統合前の静的検証

[静的検証](static-verification.json)でstrict traceability 231要件・274ノード・不完全0、生成一覧fresh、入力とZIP、checker一行差分、全record・指標と画像hashを確認。既存traceability関連39 testsを試行し、Git fixture初期化で6 failures（2 subtestsを含む）が発生した。操作は一時フォルダの`git init --quiet`、障害は`.git/config`書込みPermission denied、exit 128。設定や権限変更で回避せず依存検証を停止した。ガイドbrowser checkerはNode syntax検査のみで、実行済みではない。[公開source版照合](public-version-source-check.json)も参照。
