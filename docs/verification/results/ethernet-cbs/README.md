# CBSガイド検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-cbs`
文書状態：ローカルCLI・Viewer・サイト・追跡構造検証は合格。Project登録は権限不足で未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-15` | 初版：CBSのidle slope1変数比較、正式版CLI・独立ZIP・実Viewer・サイトの検証証跡を記録 |

## 出自と適用範囲

正式Release一覧・タグ・最新main・open Issue/PRと文書規約を2026-10-15に確認した。最新mainは`94450fa52378c155c3e8de67312e09838d2ea6d4`。正式Releaseはv1.1.4、v1.1.5はタグのみ。リポジトリ内にAGENTS.mdは存在しなかった。文書PATCHはpush単位で確定し、公開済み履歴を保持する。

新しい作業ツリー・`docs/ethernet-cbs-guide`を作り、公開済みPR97 head `2bf8fe888cb3bbbaaf14eb65936caa2743aad3f6`を通常のfast-forwardで取り込んだ。PR97由来のnav・入口・ZIP・記事・履歴を保持し、そのcommitを今回の文書改訂基準とする。PR96/97の既存branch・作業ツリーは編集していない。PR96の実装を取り込まず、CBSの製品コードも変更しない。

公開`examples/ethernet/tsn/cbs-model.json`、`cbs-workload.json`、`models/ethtsn/Main.ned`を基礎に、`tsn.outputs[0].cbs[0].idle_slope_bps`だけを変更した。[仕様](../../../specs/models/EthernetTSN詳細機能仕様書.md#scheduling)から占有672000ps、credit debt=(R−idle)*672000/Q、回復ceil(debt*Q/idle)、次SOF=release+回復を独立計算する。[既存DIR-TEST-0114](../../cases/EthernetTSN検証仕様書.md#dir-test-0114)と対象DIR-REQ-0248/0249/0250/0251・DIR-FUNC-0062のCBS責務の一部を補足する。要件・機能・試験IDを新設せず、飽和・TAS合成・全製品受入の代替としない。

## CLIと独立ZIP

[measurements.json](measurements.json)は`scripts/verify_guide_ethernet_cbs.py`で生成した。正式v1.1.4 source `b45515644dfc65c205216d564d5bf7ef00280622`のclean binaryを使用した。WSL Ubuntu 24.04 LTS x86_64・Rust1.85.0のdevビルドであり、release最適化buildの速度測定とはしない。既存binaryをSHA確認とともに再利用し、新しい並行ビルドは行っていない。

3条件×validate/run/view×repository/隔離ZIPの18操作がexit0。全manifest file hash・complete/partial false・events_exhausted/pending0、Git commit/clean・binary/source/lock hash、現行8入力hashを記録した。全transferとreceptionの時刻、CRC・wire長・tagなし、FIFO順・creditの符号/numerator/scale・回復wake・flow平均を独立期待値と照合した。隔離ZIPは全simulation objectがrepository入力と一致した。

2個目SOFは2688000/1344000/896000ps、最初のrelease creditは−504/−336/−168bit。generated/serialized/receivedは全条件2、dropped/filtered0。初回probeは出力親を未作成だったためexit4となり、親を作成して再実行した。初回checkerは2個目copyのIDをc0と誤認し失敗したため、実契約の`cbs:1/v0/c1`へ修正してfresh2で全再測定した。これらは製品不具合や合格結果に置き換えない。

## Viewer・サイト・制約

実Viewer・サイトの結果は`viewer-browser.json`、`site-browser.json`に保存する。browserは明示したfresh CLI出力と測定証拠・現行入力hashを照合し、別runへのfallbackを許さない。初回browser assertionはカードにslopeが表示されると誤認して失敗した。実カードはsigned numerator/scaleを表示し、slopeはJSON/入力で読むことを本文・checkerへ反映した。CLI期待値は変更していない。

実Chromiumで3条件を確認した。576000psでは送信器busy、672000psではcredit_negative、次wake−1psでqueue1、next wakeで2個目が送信中、最後にserialized2/received2。672000psの実390pxカード3 PNGを撮影して開き、負creditの符号・分母・open/待機理由・次wakeを目視確認した。画像は別描画や代替素材で作成していない。

最終記事のdesktop上部・390px上部・実測箇所のPNGも開き、見出し・説明文・コードブロックが既存レイアウト内で読めることを確認した。幅の広い表/コードは既存CSSの横スクロールで読む。

MkDocs1.6.1 strict、26 HTML（404込み）・1695ローカルリンク/画像/アンカー、全18 ZIP正本一致、全25ページの画像読込み/390px幅、日本語検索「回復」「傾き」が合格。既存第三者notice/hash照合と19テストも合格。対象0248/0249/0250/0251と全体のtraceability strictは231要件・60機能・274ノード、構造error0・incomplete0。構造の合格をIEEE適合や全仕様の意味的充足へ読み替えない。公開ガイドに執筆管理メモを混ぜず、絶対ホストパスは公開しない。

Projectは現行gh認証にread:projectがなく、一覧取得が拒否された。認証や権限を変更せず、アクセス拒否を回避する操作も行わない。前回のブラウザ起動障害を根拠に今回の成功を主張せず、Project登録/進行状態更新は未実施とする。Issue自動クローズを起こし得るDone操作も行わない。

制約は理想共通時計・常時open・1Gbps・単一class・同時刻2frame。credit飽和・TAS閉鎖中freeze・クラス間競合・満杯drop・途中停止を本実習で測定しない。full Rust/native HTTP suite、IEEE全規格適合、実hardware性能の証明を主張しない。管理・執筆メモは公開ガイド外へ置く。
