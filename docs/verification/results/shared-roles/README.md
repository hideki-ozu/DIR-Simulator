# 共有メモリactor実習検証記録

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.4`  
実測ソースcommit：`b45515644dfc65c205216d564d5bf7ef00280622`  
文書ID：`verification-guide-shared-roles`  
文書状態：補助記録（読者サイトの外）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-12` | 共有メモリのactor比較・隔離ZIP・実Viewer・fresh証跡・metadata・サイト検証を記録 |

基準mainは `c20e5845f39e032b6a7524efc2e26d74b4dfccdb`。正式Release/tagを再取得し、clean v1.1.4のCLIをWSL Ubuntu 24.04で使用した。正式タグと、その後のmainの提供状態を区別した。画像生成は使用せず、公開サンプルで実Viewerを撮影した。

- CLI：新規出力への18操作で、公開入力と隔離ZIPの各3条件をvalidate/run/view。変数はconsumeのactorだけ。全simulation一致、全manifestファイルのbytes/SHA-256、要求・slot・byte内容・集計を照合。validateは構成node1/channel0。
- 結果：publishは全条件0→100psに成功。readerのconsumeは150→250psに成功して01020304を返しslot free。writer/guestは150psのadmissionでaccess_denied、started/planned/outputはnull、slot readyと公開payloadを保持。全条件T400ps、events_exhausted、exit_code0、partial=false、残イベント0。
- Viewer：実Chromium149.0.7827.55、300ps、2要求、counter、consumeのactor/status/reason/output、slot free/readyをassert。新規PNG3枚の実画素を確認。actor表示とNED構成数は別で、図は操作許可の証拠ではない。
- fresh証跡：両browser checkerへ明示したCLI出力の9ファイルhashを記録。有効な旧成功出力があっても引数省略・不存在・選択Viewer欠落を拒否する6負例に成功。
- サイト：MkDocs strict、17 HTML / 949ローカルリンク・画像・アンカー、管理メモ除外。実Chromiumで404以外16ページの390px幅・全画像読込・外部要求なし、日本語「共有」「権限」の記事到達をassert。desktop・390pxの実画素も確認した。
- 配布：既存8 ZIPを維持し、新規8入力ZIPを含む全9 ZIPの入力一致を確認。
- traceability：回帰40件、全体strict、DIR-REQ-0206〜0209の適用範囲strict成功（231要件・60機能・274ノード、欠落0）。最終文書をstage後に生成一覧を更新しfreshnessを確認する。
- metadata：`python scripts/check_shared_document_metadata.py --source-only` で実生成器のパーサーと源文書を照合し、生成後は同コマンドのsource-only指定なしでMarkdown/HTML双方の新規2文書の版・対象タグを照合する。`python scripts/verify_shared_metadata_regression.py` は隔離コピーで正常→括弧付きcommit拒否→復元成功の3段階を確認し、前回の「未記載」見落としを検出する。
- ライセンス：回帰9件、guide原本8・供給payload37・lock package42・crate原本89・source bundle10の対応検証成功。法的承認を意味しない。

[measurements.json](measurements.json)、[viewer-browser.json](viewer-browser.json)、[site-browser.json](site-browser.json)に公開証拠を保存する。raw出力・ローカルパス・一時サイト画面はGit対象外。最終fresh出力のパス・exact head CI・Projectの非完了進行状態はPR本文で確認する。

制約：slot1個・公開/取得各1回。競合・複数slot/consumer・full/empty・途中打切り・両役割登録は未検証。Windows native・CPU cache/fence・物理共有RAM・OS認証/security分離・実機性能も未検証。マージ・auto-merge・close・デプロイは実施しない。
