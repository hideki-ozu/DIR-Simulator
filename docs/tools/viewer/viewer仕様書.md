# viewer仕様書

文書バージョン：`1.1.2`
対象GitHubバージョン：`main @ 7bb9bfc`
予定公開版：`v1.1.2`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-04` | CAN FDの証跡・位相時間と媒体の試行・jam・backoff・PHY表示、停止と巻き戻しの再現範囲を追加 |
| `1.1.1` | `2026-10-04` | v1.1.1向けVLAN profileの読込対応と詳細仕様への参照を追加 |
| `1.1.0` | `2026-10-04` | 初版公開：CAN/GW viewerの表示・再生・状態復元・検証条件を集約。説明範囲とEthernet資料への参照を明示 |

文書ID：`tool-viewer-spec`

文書状態：公開草案（CAN/GW画面の初版）

本書はCAN/GW画面の説明を対象とする。v1.1.0で追加したEthernet画面は[Ethernet負荷・QoS詳細機能仕様書](../../specs/models/Ethernet負荷・QoS詳細機能仕様書.md)と[READMEのEthernet実行例](../../../README.md#ethernetサンプルを実行する)を参照する。v1.1.1向けPRのVLAN・静的multicast画面は[EthernetVLAN・マルチキャスト詳細機能仕様書](../../specs/models/EthernetVLAN・マルチキャスト詳細機能仕様書.md)を参照する。

本書はCAN/GWの結果ビューアについて、外部から見える振る舞い、実装上の復元方法、受け入れ確認をまとめる。操作手順は[viewer取扱説明書](viewer取扱説明書.md)に記す。結果JSONの項目定義、Gatewayの転送・容量・時刻の規則は、それぞれ[結果詳細機能仕様書](../../specs/結果詳細機能仕様書.md)、[GWモデル詳細機能仕様書](../../specs/models/GWモデル詳細機能仕様書.md)、[GWモデル詳細設計書](../../design/GWモデル詳細設計書.md)を正本とする。

## 1. 対象と入出力

`dir-simulator view --input results.json --output viewer.html` は既存の結果JSONを読み、CSS、JavaScript、入力JSONを埋め込んだ単独HTMLを作る。コマンドはブラウザを起動しない。入力結果・manifestを変更せず、既存の出力ファイルと同名のシンボリックリンクを上書きしない。出力先の親ディレクトリは存在する必要がある。生成したHTMLはローカルブラウザで開け、外部通信を行わない。[CLI実装](../../../crates/dir-simulator/src/main.rs)と[HTML生成](../../../crates/dir-simulator/src/tool/viewer.rs)がこの境界を担う。

本書で説明する入力はschema 1の`can.cc.ideal.v1`とschema 2の`can.cc.multibus.v1`の結果である。viewerはschema 2の`ethernet.l2.store-forward.v1`・`ethernet.l2.qos.v1`と、v1.1.1向けPRの`ethernet.l2.vlan.v1`にも対応し、別のEthernet画面へ切り替える。CLIはJSON構文、schema版、simulationの形、schema 2のprofileとmodel_recordsの存在を検査する。ブラウザ側の[復元モデル](../../../crates/dir-simulator/src/tool/viewer/assets/model.js)が要求・受信・Gateway行の項目、時刻、参照関係など表示に必要な整合性を検査する。したがってCLIがHTMLを作成できても、レコードが不正ならブラウザで読込エラーになる。任意のschema 2モデルを汎用表示する機能ではない。

## 2. 時刻と状態の仕様

時刻はps単位の十進文字列を`BigInt`で扱い、表示単位はµs、ns、ps、ms、sから選べる。時刻指定は1 ps精度で入力する。選択時刻には、その時刻までに確定した記録をすべて反映する。同時刻のスケジューラ内部イベント順は再生しない。予定EOF・予定解放は状態遷移の完了とみなさない。部分結果はバッジで示し、記録された確定範囲のみを表示する。

選択時刻の要求・受信・ノード・バス・GW RX保持・TX容量待機の状態は、到達済み時刻と計測記録から毎回復元する。タイムラインと詳細欄の時刻は実行全体の記録、件数と状態は選択時刻の値である。巻き戻しでも同じ規則を適用する。結果に`metadata.topology.controllers`がある場合は、通信していないControllerを含むBus所属とTX/RX経路遅延を接続の正本として使う。古い結果にtopologyがない場合は要求・受信から接続を復元し、単一Busで記録のないノードに推定接続の印を付ける。GatewayのController所属と論理経路はschema 2の正規化設定を使う。

## 3. 表示・再生の仕様

画面にはノード・バス接続図、通信タイムライン、選択時刻の集計とノード状態、要求一覧・詳細、schema 2のGateway転送とRX保持を表示する。要求詳細では生成、SOF、EOF、解放、受信などの時刻と元レコードを確認できる。Gatewayでは元要求・親・コピー、処理中、TX受理待ち、RX保持・破棄、hop破棄、経路不一致、終端への経路遅延を追える。経路遅延はGW portを除く終端Controllerの確定received時刻と元要求generated時刻の差である。

連続再生の1×は観測期間全体を約8秒で進める表示速度で、物理的な通信時間とは異なる。連続再生ではメッセージを移動させず、Controller→Busの送信線を青、Bus→Controllerの受信線を橙、通常の接続を灰にし、Gateway内部も入口→内部転送点→出口の論理経路を強調する。線には方向矢印を付け、直近の完了は観測期間の1/40だけ残像として示す。停止中に時刻指定やスライダーで移動した場合は、選択時刻での論理進捗と直近の完了を示す。

前後のイベント時刻ボタンと左右キーは、異なる記録時刻へ移動する。移動先の直前の記録時刻から移動先までの区間を再表示するため、前進と後退で同じ移動先なら同じ通信方向の動きになる。最初の記録時刻には再表示する区間がない。一区間の各段階は約0.7秒で、CANの送信→受信だけなら約1.4秒となる。受信だけの区間は受信から始まる。親CAN受信→Gateway内部送信・受信→出口CAN送信・受信の因果順に段階を並べ、分岐先ごとに経路を分ける。Gateway内部転送点は論理経路の中央に置く。出口TX受理が未確定の枝には内部受信を描かず、TX容量待機は入口側に示す。遅延ゼロの同報受信も各受信先に示す。アニメーション中の選択時刻と件数は移動先の確定状態であり、動画は物理伝搬時間の再現ではない。

タイムラインの描画上限は500区間、要求一覧は1ページ75行、Gateway転送は受信時刻順の最新500行である。省略件数または最新行表示を画面に示す。検索・状態フィルタ・タイムライン拡大・ページ移動で対象を探せる。描画上限や検索条件は構成図の送受信表示と状態集計の母数を変えない。

## 4. 実装上の境界と受け入れ確認

HTML生成は同一ディレクトリに一時ファイルを作り、完成後のハードリンクで新規出力を公開する。埋め込みJSON中の`<`とスクリプト終了に関わる文字を安全に符号化する。[モデル](../../../crates/dir-simulator/src/tool/viewer/assets/model.js)は入力の確定時刻から状態とステップの依存順を算出し、[画面処理](../../../crates/dir-simulator/src/tool/viewer/assets/app.js)は再生と描画上限を適用する。構成・結果データの意味をビューア独自に変更しない。

| 確認対象 | 受け入れ観点 | 対応する既存試験 |
| --- | --- | --- |
| CLIと出力保護 | 不正入力・既存出力・欠けた親ディレクトリで元ファイルを保ち、正常時は埋込みHTMLを生成する | [viewer CLI試験](../../../crates/dir-simulator/tests/viewer_cli.rs) |
| 復元と境界 | schema 1/2、整数時刻、確定時刻、部分結果、Gatewayの親子・保持・待機・分岐を正しく復元する | [モデル試験](../../../tests/viewer_model.test.cjs) |
| ブラウザ操作 | ファイル読込、時刻移動、構成図、前後ステップ、表示上限、外部通信の有無を確認する | [ブラウザ試験](../../../tests/viewer_browser.cjs) |

既存試験の実施範囲と取得時のソースは[GWモデル検証仕様書](../../verification/cases/GWモデル検証仕様書.md)と[ビューア接続・受信の記録](../../verification/results/viewer-rx-gateway-2026-10-03.json)を参照する。本書の作成自体を製品試験の再実施や新たな適合証拠とは扱わない。

### 確認コマンド

以下はリポジトリルートから実行する。Node.jsとPlaywrightは試験用であり、生成HTMLを利用するブラウザには不要である。

```bash
cargo test --locked -p dir-simulator --test viewer_cli
cargo test --locked -p dir-simulator --lib tool::viewer::tests
node --test tests/viewer_model.test.cjs
```

ブラウザ操作の自動確認を行う場合は、テスト用のPlaywrightとChromiumを配置して実行する。

```bash
cargo build --locked -p dir-simulator
npm install --prefix tmp/viewer-tests playwright@1.62.1
./tmp/viewer-tests/node_modules/.bin/playwright install chromium
NODE_PATH="$PWD/tmp/viewer-tests/node_modules" node tests/viewer_browser.cjs
```

## 開発中profileの表示

`ethernet.l2.store-forward.v2`と`ethernet.l2.100base-t1.v1`はEthernet画面へ切り替える。物理linkのmode/duplex/role/PHY遅延、試行数・衝突数、試行ごとのSOF・collision・jam・backoff、MAC/MDIの予定時刻と成功実績を表示する。衝突試行から正常受信を作らず、停止時の未到達EOF・jam終了・arrivalを補完しない。前後ステップは移動先の直前区間を同じ向きで再表示する。

`can.fd.precomputed.v1`はController→Bus→受信先の画面を使い、二速度・DLC・BRS・外部位相bit数・evidence・binding SHA-256を詳細へ表示する。`fidelity=externally-precomputed-phase-bits`、`wire_validation=structural-only`を明示する。静的frame行があっても生成時刻が停止境界以後なら送信要求を作らない。任意profileの汎用codecではなく、対応する3種のFDレコードを検証して共通の記録時刻再演へ接続する。

この追加は開発中ソースの実装であり、公開版と文書版は次回push準備時に確定する。
