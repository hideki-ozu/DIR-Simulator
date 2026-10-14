# 静的multicastガイド検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-static-multicast`
文書状態：ローカルのCLI・Viewer・サイト・追跡構造検証は合格。Project登録は権限不足で未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-14` | 初版：静的group出口の1変数比較と独立ZIP・実Viewer・公開ガイドの検証証跡を記録 |

## 対象・出自・期待値

実測sourceは正式v1.1.4の`b45515644dfc65c205216d564d5bf7ef00280622`。最新main `94450fa52378c155c3e8de67312e09838d2ea6d4`から独立した`docs/ethernet-multicast-guide`で文書を作成した。公開`examples/ethernet`のMain/VlanDemo NEDとvlan-multicast workload、model-vlanを基礎に、s1のgroup出口リストだけを変更した。正式Release一覧を2026-10-14に再取得し、v1.1.4が最新正式Release、v1.1.5はタグのみと確認した。

期待値は[静的group転送仕様](../../../specs/models/EthernetVLAN・マルチキャスト詳細機能仕様書.md#forwarding)と[観測仕様](../../../specs/models/EthernetVLAN・マルチキャスト詳細機能仕様書.md#observations)から計算した。登録空列はmulticast_no_egress、未購読Endpointはmulticast_not_subscribed、cのみ購読済み。1copyはMAC68byte・直列化608000ps・IFG込み704000ps、Link伝搬1000ps・Switch処理2000psで独立に時刻を求めた。測定した出力を期待値の生成器として使わない。

既存`DIR-REQ-0226/0227/0229`、`DIR-FUNC-0056/0057`、`DIR-TEST-0103/0104`の静的group転送・copy関係・結果/Viewer契約に適用する。本記事はそれらの一部シナリオの補足実習であり、要件・仕様・実装の変更や全製品試験の代替ではない。traceabilityには新しい架空の機能IDを追加しない。

## CLIと配布入力

[measurements.json](measurements.json)に正式版のcommit・clean状態・compiler・binary/source/lock SHA256、9入力hash、3条件の全simulationと独立ZIP再現を保存する。WSL Ubuntu 24.04 LTS x86_64・Rust 1.85.0、独立checkoutで`cargo build --offline --locked -j 1 -p dir-simulator`したdev binaryを使用した。読者手順のreleaseビルドも同一タグ・ソースを指定するが、本実測をrelease optimization buildの証拠とはしない。

3条件×validate/run/view×repository/独立ZIPで18 CLI操作、全exit0。各runのcomplete/partial false・events_exhausted/pending0と全manifest file hashを確認した。source frame1、serialized1/2/7、forwarded0/1/3、received0/1/1、filtered1/0/3、drop0。全copyの時刻・wire長/CRC・tag・親子参照・受信理由と集計を検査し、独立ZIPの全simulation objectがrepository入力と一致した。

初回`multicast-first`はWindows作成worktreeをWSL Gitがdirtyと判定したため、provenance assertionで失敗した。この結果は採用していない。WSL Gitで正式タグを別checkoutへ取得したclean binaryで`multicast-clean`へ再実行し合格した。元の失敗を成功に書き換えず、製品の動作不良とは扱わない。Windows側の初回venv依存取得もsandboxのDNS失敗で未完了だったため、作業内venvへ承認済みネットワークで依存を導入してから実施した。

## Viewer・サイト・管理の確認

Viewer・サイトの実施結果は[viewer-browser.json](viewer-browser.json)、[site-browser.json](site-browser.json)で記録する。実Chromiumの3条件を4000000psへ移動し、copy数・aのsource parent・s1→s2のincoming parent・bの未購読理由・cの完了受信を確認した。初回browser assertionは実画面が英語statusや子reception JSONを表示すると誤認して失敗した。実際の日本語表示（転送判断済み／受信完了）と、元frame全体の受信一覧・選択transferだけのJSON表示へ本文とcheckerを修正し、再実行で合格した。CLI期待値は変更していない。

全24ページを実ブラウザで巡回し、画像読込み・390px横はみ出しなし・日本語検索「静的」「購読」を確認した。25 HTML（404込み）と1604ローカルリンク/画像/アンカー、全17配布ZIPの正本一致、MkDocs1.6.1 strict、第三者notice/hash確認とその既存19テストが合格。全体traceability strictは231要件・60機能・274ノード、構造error0・incomplete0。対象0226/0227/0229のstrictもerror/incomplete0。構造の合格を規格適合や全仕様の意味的充足と読み替えない。

公開PNG2枚と記事desktop/390px上部・実測表の実画像を開いて目視した。文字・経路・親copy・受信理由を確認し、390pxの表は既存CSSの横スクロールで読む。撮影は正式CLIが生成したViewerだけを採用し、旧記事画像・代替画像を使わない。MkDocs buildには管理メモを含めていない。CLI、Viewer、サイトの絶対ホストパスを公開証跡へ保存しない。

Projectは現行gh認証に`read:project`がなく、`gh project list`および読み取りGraphQLがINSUFFICIENT_SCOPESで拒否された。既存ログインブラウザによる代替も試みたが、computer-useのkernelが必要なWindows platform directoryを見つけられず起動に失敗した（HRESULT 0x80070003）。認証・権限設定は変更していない。登録・状態更新は未実施。Issueを閉じ得るDone操作も実施していない。

制約：静的表・単一tag・非巡回tree・競合なし1フレームのみ。未登録group、部分満杯、途中停止、動的snooping/STPや上位protocolを本実習では測定しない。full Rust/native HTTP suiteは記事変更のローカル検証として実施していない。PR96のbranch/作業ツリー・CodeRabbit・外部エージェントを操作していない。
