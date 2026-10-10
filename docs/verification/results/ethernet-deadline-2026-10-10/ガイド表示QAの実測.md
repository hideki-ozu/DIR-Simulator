# ガイド表示QAの実測

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
文書ID：`evidence-deadline-guide-browser`  
文書状態：通常環境の実測証跡を受領し、hashと実画像を確認済み

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-10` | 第3push。固定第2headのガイド表示・日本語検索・390px実測とProject登録を追補 |

## 対象と結果

固定head `f5ef3cb08b7b6218a061deade085ae6dd7b264de` のsource ZIPを新規空フォルダへ展開。96 member、manifestの95ファイル、元ZIP SHA256 `339805c3a69c3a16caf0649d0be3acadf661908256e2dc672ad09f87741e9d70` を受領後にもbyte/hash照合した。既存MkDocs 1.6.1・Playwrightを通常VSCode環境で使用し、追加インストールや設定変更はない。製品ビルド・試験・18操作のCLI実測・Viewer撮影は再実行していない。

MkDocs strict、リンク検査、ガイドbrowser検査の3操作はexit 0。11 HTML、526リンク・画像・anchorが合格。Chromium 151.0.7922.34で本文4画像が読込成功、390px viewport/document幅は共に390、page/console errors・request failures・external requestsは全て0。日本語検索「期限」「配送」は両方で当該記事が現れた。[最終まとめ](guide-browser-f5ef3cb0/verification-summary.json)、[browser詳細](guide-browser-f5ef3cb0/browser-evidence/report.json)を参照。

元execution.jsonの`passed_automated_visual_review_pending`は取得時の履歴として保持する。後続の目視記録と最終まとめはpassed。受領後、下記4枚の実PNGも開いて確認した。デスクトップ/390px本文の全体構成と4画像が表示され、検索画像には実際の検索語と記事名がある。390pxの広い表は内部スクロールで読む。画像は加工していない。縮小された全長画像だけで細字の全てを読めると主張しない。

- [デスクトップ本文](guide-browser-f5ef3cb0/browser-evidence/deadline-guide-desktop.png)
- [390px本文](guide-browser-f5ef3cb0/browser-evidence/deadline-guide-390px.png)
- [「期限」検索](guide-browser-f5ef3cb0/browser-evidence/deadline-search-期限.png)
- [「配送」検索](guide-browser-f5ef3cb0/browser-evidence/deadline-search-配送.png)

絶対パスだけを公開コピーではplaceholderへ置換した。[公開コピー一覧](guide-browser-f5ef3cb0/public-copy-index.json)のoriginal hashとpublic hashを区別する。原本は受領フォルダに保持する。既存checkoutのhead/status/refs/worktrees、ZIPと入力は実行者のbefore/after記録で不変。source95ファイルは受領者も原ZIPと照合した。

## 統合状態と限界

親タスクが[Project「DIR取説整備」](https://github.com/users/hideki-ozu/projects/1)でDraft PR #50の登録1件とStatus「執筆中」を確認し、PRの再読で保存を確認した。これは親のUI確認結果であり、この実行環境でProject操作を重複していない。Issue closeや完了状態への変更はない。

第3pushはこの証跡・README・生成一覧のみを変更する。公開記事、nav、MkDocs設定、ガイド画像、入力、browser checkerは実測した第2headのまま保持する。そのため表示実測headと最終追補headは区別する。v1.1.3のCLI実測、全仕様適合、公開Site反映を意味しない。先行静的試験のsandbox Git fixture失敗は旧記録に保持し、成功へ書き換えない。
