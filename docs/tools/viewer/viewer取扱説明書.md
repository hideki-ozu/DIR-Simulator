# viewer取扱説明書

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-04` | 初版公開：単独HTMLの作成、CAN/GW画面の操作・エラー対処を集約。必要な資産一式とEthernet資料への参照を明示 |

文書ID：`tool-viewer-manual`

文書状態：公開草案（CAN/GW画面の初版）

本書はCAN/GW画面の説明を対象とする。v1.1.0で追加したEthernet画面は[Ethernet負荷・QoS詳細機能仕様書](../../specs/models/Ethernet負荷・QoS詳細機能仕様書.md)と[READMEのEthernet実行例](../../../README.md#ethernetサンプルを実行する)を参照する。

本書は結果ビューアの作成と操作を説明する。表示の契約、内部の復元方法、受け入れ確認は[viewer仕様書](viewer仕様書.md)を参照する。以下のコマンドはリポジトリのルートから実行する。

## 1. 結果を開く

既存の`results.json`があれば、CLIで単独HTMLを作る。出力先には存在しないファイル名を指定する。

```bash
cargo build --locked --release -p dir-simulator
./target/release/dir-simulator view \
  --input results/results.json \
  --output results-viewer.html
```

`results-viewer.html`をブラウザで開く。WSLからWindowsの既定ブラウザで開く場合は次を実行する。

```bash
explorer.exe "$(wslpath -w "$PWD/results-viewer.html")"
```

サンプルの結果を先に作る場合は、未存在の出力ディレクトリを指定して`run`を実行し、その`results.json`を`view`へ渡す。

```bash
./target/release/dir-simulator run \
  --config examples/gateway/fanout.ini \
  --output results-gateway
./target/release/dir-simulator view \
  --input results-gateway/results.json \
  --output gateway-viewer.html
```

HTMLには結果と入力スナップショットが埋め込まれ、ブラウザでの表示にサーバーや追加パッケージは不要である。元の結果とmanifestは変更されない。HTMLを別の環境へ渡す場合も同じ記録を表示できる。CLIはブラウザを自動起動しない。

代わりに[ビューア本体](../../../crates/dir-simulator/src/tool/viewer/assets/index.html)を直接開き、「results.json を選択」から結果を読むこともできる。この方法では同じフォルダの`style.css`、`model.js`、`ethernet-model.js`、`ethernet-app.js`、`app.js`を一緒に保つ。いずれの方法もファイルはブラウザ内で処理され、外部通信を行わない。

## 2. 時刻と通信を読む

「▶ 再生」で観測期間を連続再生し、「Ⅱ 一時停止」で止める。速度は0.25×、0.5×、1×、2×、4×から選ぶ。1×では観測期間全体を約8秒で表示する。これは画面上の速度で、実通信の速度ではない。連続再生中はメッセージの移動ではなく、送信線を青、受信線を橙、通常の接続を灰にして方向矢印を表示する。直近の完了も短時間だけ強調する。

スライダー、時刻指定、タイムラインのクリックで停止位置を選べる。時刻はµs、ns、ps、ms、sを切り替えられ、内部ではps整数の精度を保つ。時刻指定欄には選択した単位で0以上の数値を入力する。1 psに換算できない小数や観測範囲外の値は入力エラーになる。集計・ノード状態は選択時刻までに確定した内容で、タイムラインと詳細欄の時刻は実行全体の記録である。「部分結果」が出た場合も、未確定の予定EOF・予定解放を完了として表示しない。

「前のイベント時刻」「次のイベント時刻」または左右キーで記録時刻ごとに移動する。同じ時刻の記録は一つの時刻として扱い、内部イベントの順番は再現しない。どちらの方向から移動しても、移動先の直前の記録時刻から移動先までを同じ方向で再表示する。最初の記録時刻には再表示区間がない。送信はController→Bus、受信はBus→Controllerへ各約0.7秒で動き、両方ある区間は計約1.4秒になる。受信だけの区間は受信から始まる。Gatewayを通ると親CANの受信、Gateway入口→内部転送点→出口、出口CANの送受信へ進み、分岐ごとに経路を分ける。出口TX受理前の枝には内部受信が出ず、TX容量待機は入口に表示される。遅延ゼロの同報受信は受信先ごとに動く。アニメーション中の件数は移動先の確定状態である。

「ノードとパケットの流れ」ではControllerとBusの接続、Gatewayを囲むブロック、内部の論理経路を確認する。構成図の線・メッセージを選ぶと要求詳細へ進める。古い結果でtopologyがない場合、通信記録から復元した接続に加え、通信記録のないノードの接続は「推定CAN接続」として区別される。狭い画面ではGateway内のControllerが縦並びになる。

## 3. 要求とGatewayを調べる

通信タイムラインはノード別TX/RX、バス占有・間隔、送信待機、GW RX保持、破棄を示す。「＋」「−」「全体」で表示範囲を調整し、区間を選ぶと要求詳細へ進む。描画は最大500区間で、省略数を表示する。

要求一覧はID・ノード検索、現在時刻の状態フィルタ、75行ずつのページ移動に対応する。行を選ぶと、生成・SOF・EOF・解放・受信時刻と元レコードを見られる。時刻が「未確定」なら到達記録がない。予定時刻は実到達と区別する。

schema 2では「Gateway転送」にRX入力Controllerごとの容量・保持数・破棄数、経路ごとのGW処理・TX容量待機・コピー生成・hop破棄・RX破棄・経路不一致を表示する。コピー要求を選び、元要求・親要求との関係や終端Controllerまでの経路遅延を確認できる。転送一覧は受信時刻順の最新500行を表示する。表示上限や検索条件は構成図の動きと状態集計の母数を変えない。

## 4. 別の結果とエラー

「結果ファイルを開く」から別の`results.json`を選ぶか、画面へ1ファイルをドロップして切り替える。本書で説明する結果はschema 1の`can.cc.ideal.v1`とschema 2の`can.cc.multibus.v1`である。schema 2の`ethernet.l2.store-forward.v1`と`ethernet.l2.qos.v1`を開くとEthernet画面へ切り替わる。未対応のschema/profileや不正なレコードは読込エラーになる。

CLIで「Viewer output already exists」と表示されたら、新しい出力ファイル名を指定する。親ディレクトリがない場合は先に作成する。入力JSONが読めない、構文が不正、schemaが対象外の場合は入力側を確認する。HTMLは作成できてもブラウザで読込エラーが出る場合は、結果の要求・受信・Gatewayレコードまたは時刻の整合性を確認する。結果を編集して直す前に、生成元の[結果仕様](../../specs/結果詳細機能仕様書.md)と[GW仕様](../../specs/models/GWモデル詳細機能仕様書.md)を参照する。
