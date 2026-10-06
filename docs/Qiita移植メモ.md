# Qiita移植メモ

文書バージョン：`1.1.1`  
対象GitHubバージョン：`v1.1.3`  
文書ID：`guide-portability`  
文書状態：執筆者向け管理資料。GitHubで保存し、MkDocs公開対象には含めない

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.1` | `2026-10-06` | 執筆者向け文書として公開ガイドの対象外へ移動し、参照先を更新 |
| `1.1.0` | `2026-10-05` | 初版：公開サンプルの実行・実画面・条件変更実験、GitHub Pages公開とMarkdown再利用を整備 |

## GitHubの説明をQiitaでも再利用できるか

できます。このガイドは標準Markdownを正本にし、通常の見出し、表、リスト、相対画像、言語名付きコードフェンスで書いています。[QiitaのMarkdown公式案内](https://help.qiita.com/ja/articles/qiita-markdown)はGitHub Flavored Markdown準拠を説明しています。MkDocs固有admonition・タブ・include構文は本文で使っていません。検索とナビは`mkdocs.yml`側に置きます。

GitHubへ保存しただけではQiitaへ自動同期されません。[公式Qiita CLI](https://github.com/increments/qiita-cli)と[GitHub連動の公式記事](https://qiita.com/Qiita/items/32c79014509987541130)による投稿管理は別途設定が必要です。今回CLIの投稿設定・トークン取得・Qiita投稿は行っていません。

## 移植する手順

1. `docs/guide`の対象ページを原稿としてコピーします。原稿の変更元はGitHub側に揃えます。
2. ページ見出しをQiita記事タイトルへ移し、冒頭の文書版・対象版・履歴は出典欄へ短く整理しても構いません。対象v1.1.3と元原稿URLは残します。
3. 相対画像を、画像を公開した絶対URLへ変えます。Qiitaへ画像をアップロードする場合はQiita側で得たURLに変えます。ローカルパスや`file://`は読者から見えません。
4. ページ間相対リンクを、公開ガイドURLまたは公開済みQiita記事のURLへ変えます。未公開Qiita記事への仮リンクは作りません。
5. コードフェンスの`bash`、`ini`、`json`、`text`を保ち、表・改行・画像幅をQiitaプレビューで確認します。
6. プロジェクトの著作権・選択したライセンスの通知、第三者素材の通知を保持します。投稿前に秘密や個人情報が混入していないか確認します。

例：GitHub Pagesの公開先は`https://hideki-ozu.github.io/DIR-Simulator/`です。初回公開はmainへのマージ後です。公開後は次の絶対URLを使えます。Qiitaの画像アップロードを使う場合は、アップロードで得たURLへ置き換えます。投稿前に、読者がログインせず画像とリンクを開けることを確認してください。所有者限定の別Siteやローカル画像パスは、Qiita読者への公開リンクとして使えません。

```markdown
![最小CAN例](assets/guide-minimal-viewer.png)
[CANの調停](CANの調停.md)
```

```markdown
![最小CAN例](https://hideki-ozu.github.io/DIR-Simulator/assets/guide-minimal-viewer.png)
[CANの調停](https://hideki-ozu.github.io/DIR-Simulator/CAN%E3%81%AE%E8%AA%BF%E5%81%9C.html)
```

`scripts/export_guide_qiita.py`は相対リンクを指定の公開base URLへ変換し、ローカル投稿候補を出力します。元の正本を変更せず、投稿はしません。出力候補は投稿前の下書きです。GitHub Pagesの初回公開完了と、すべての画像・リンクへ読者がアクセスできることを確認してください。所有者限定Siteをbase URLに指定した場合は、公開できる画像・リンクへ置き換えます。

```bash
python scripts/export_guide_qiita.py --base-url https://hideki-ozu.github.io/DIR-Simulator/ --output build/qiita
```

## ライセンスと出典

DIRの`LICENSE`は、別個の通知のないプロジェクト作成文書・テンプレート・生成図をMITまたはApache-2.0の選択ライセンスの対象としています。本ガイドと公開サンプルのViewer画像もプロジェクト作成素材として同じ通知を保持します。

> Copyright (c) 2026 DIR Simulator contributors. Licensed under MIT OR Apache-2.0.

転載時は元リポジトリの[LICENSE](https://github.com/hideki-ozu/DIR-Simulator/blob/v1.1.3/LICENSE)、[MIT全文](https://github.com/hideki-ozu/DIR-Simulator/blob/v1.1.3/LICENSE-MIT)、[Apache-2.0全文](https://github.com/hideki-ozu/DIR-Simulator/blob/v1.1.3/LICENSE-APACHE)を参照できるようにし、選択した条件に沿って必要な通知を保ちます。

静的ガイドの検索・テーマ付属資産の通知は[ライセンス出典一覧](guide/assets/licenses/sources.txt)へ保持しています。

第三者素材はこの許諾に含まれません。出典を書くことだけで転載の許可になるとは限らないため、個別の利用条件を確認します。[Qiitaの無断転載に関する公式説明](https://help.qiita.com/ja/articles/about-unauthorized-reproduction)も参照してください。OMNeT++比較の削除済み成果物は今回の原稿・画像・結果に使っていません。

## 更新を続けるとき

GitHub正本とQiita記事の関係を対応表で管理し、記事へ対象タグ・原稿版を記します。GitHub側の本文改訂を確認してからQiitaへ反映します。DIRの文書改訂番号はpush単位で確定する規約なので、保存やローカルレビューごとに番号を増やしません。
