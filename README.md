# DIR Simulator

文書バージョン：`1.0.2`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.2` | `2026-10-03` | Issue #4対応：OMNeT++を用いた比較結果・添付資料への案内を削除し、DIR単独の検証範囲を明確化 |
| `1.0.1` | `2026-10-03` | レビュー修正：診断保持API、Viewer試験の前提、保存済みOMNeT比較手順を明確化 |
| `1.0.0` | `2026-10-03` | GW・複数CAN、RX保持、Busゲート名自由化、ソース分割、ビューアの送受信と前後ステップ、比較証跡を反映。文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | Classical CANのCLI・ライブラリ、結果ビューア、実行例、OMNeT++比較の利用方法を集約し、v0.1公開版を確定 |

文書ID：`readme`

対象タグは`v1.0.0`、文書版は`1.0.2`です。Cargoパッケージ版は`0.1.0`です。

**DIR = Definition（定義）、Initialization（初期化）、Runtime（実行）**

DIR Simulatorは、CAN／CAN FD・Ethernet・SoC通信・メモリ・IPCと接続デバイスを対象としたRust製の離散イベント型シミュレータです。
次の3層モデルを採用しています。

1. **Definition（定義）** — NEDの構造記述の一部に対応
2. **Initialization（初期化）** — 必要最小限の独自INI仕様によるシナリオ設定とパラメータの上書き
3. **Runtime（実行）** — Rustネイティブのシミュレーションモジュールと実行エンジン

## 目標

CANノードと共有バスの通信を仮想時刻上で実行し、負荷・ビットレート・容量の違いによる遅延、競合、バッファ使用量と性能を比較します。
構造をNED、実験条件をINI、振る舞いをRustで記述し、時系列・集計値をCSVとJSONで取得できます。

基準モデルはClassical CANです。v0.1は単一CANを提供し、v1.0.0ではGW・複数CANバスの実装と製品試験を追加しています。AXIのManager・Interconnect・Ram、EthernetのEndpoint・Link・Switchも要件から詳細設計・検証まで規定し、1.0.0以後の追加計画としています。各profileの初期機能を固定し、追加機能は版付き登録契約で拡張します。
初期モデルは標準11bit・拡張29bitのClassical CANデータフレームを扱う`can.cc.ideal.v1`です。内容からCRC・stuff bitを求め、理想ACKを扱います。詳細は[CANモデル仕様](docs/specs/models/CANモデル詳細機能仕様書.md)、判断履歴は[解決済みTBD台帳](docs/要件定義書.md#6-未確定事項不足の管理)に記載しています。

## CANサンプルを実行する

v1.0.0では、単一CAN・複数CAN共通でBusゲート名の制約撤廃を反映しています。Busの入力・出力の役割は`input`／`output`、ペアは同じControllerへの接続経路から決まります。サンプルは入力`rx_a`・出力`tx_a`などを使いますが、接頭辞や接尾辞の一致は必須ではありません。旧形式の有効な入力`tx_a`／出力`rx_a`も読み込めます。Controllerの`tx`／`rx`は維持します。以下は現在のソースをビルドして実行してください。公開済みv0.1の仕様はその時点のものです。

Linux / WSLでRust 1.85.0を使用します。`rust-toolchain.toml`でツールチェーン、`Cargo.lock`で依存版を固定しています。Rust導入済みのシェルで、リポジトリのルートから実行してください。

```bash
cargo build --locked --release -p dir-simulator
./target/release/dir-simulator validate --config examples/can/baseline.ini
./target/release/dir-simulator run --config examples/can/baseline.ini --output results
```

`results`は新規または空のディレクトリを指定します。親ディレクトリは事前に作成してください。再実行時は別名を指定します。インストールする場合は`cargo install --locked --path crates/dir-simulator`を使用できます。

| サンプル | 条件 | 確認する動作 |
| --- | --- | --- |
| [baseline.ini](examples/can/baseline.ini) | 3ノード、500kbps、1ms周期、容量64 | 同時生成の仲裁と、待機解消後のアイドル |
| [contention.ini](examples/can/contention.ini) | 200us周期、容量64 | 高負荷による送信待ちとキュー増加 |
| [overload.ini](examples/can/overload.ini) | 50us周期、容量2 | キュー満杯時の新着要求破棄 |

同じNED構成を使用し、INIでビットレート・キュー容量・処理遅延、対応するJSONでID・データ・送信周期を変更できます。拡張29bitフレームはJSONの`format`に`extended`を指定します。INI/JSONの未知キーや、不正な型・単位・接続・ID所有者重複は実行前に拒否します。

出力は`manifest.json`、`results.json`、`events.csv`、`summary.csv`、`diagnostics.jsonl`です。manifestは最後に公開し、各データファイルのSHA-256を保持します。JSONの`simulation.requests`で要求ごとの状態・SOF/EOF、`receivers`で受信時刻、`records`と`summary`で37種類の指標の観測・集計を確認できます。時刻の単位はps、整数値は精度保持のため十進文字列です。

```bash
python3 - <<'PYRESULT'
import json
from pathlib import Path
result = json.loads(Path("results/results.json").read_text())
for row in result["simulation"]["summary"]:
    if row["target"] in ("$all", "Main.bus") and row["metric"] in (
        "generated", "success", "dropped", "unfinished", "received",
        "bus_utilization", "arbitration_wait_mean_ps",
    ):
        print(row["target"], row["metric"], row["value"])
PYRESULT
```

CLIの終了コードは正常0、入力・準備失敗2、実行失敗3、出力失敗4です。正常終了の`time_limit`でも未完了要求はあり得ます。`max-events`等による実行失敗時には、最後に確定した要求・受信状態を`partial=true`で出力します。準備失敗時は診断をstderrへ返し、結果ファイルは作成しません。実行失敗後に結果の保存も失敗した場合は終了コード4となり、先の実行診断と最後の出力診断を順にstderrへ返します。

ライブラリは`dir_simulator::prepare(&Path)`と`dir_simulator::run(prepared, &Path)`から同じ処理を利用できます。`run`のエラー型は従来の`Diagnostic`を維持し、保存失敗前の診断もメッセージへ残します。診断を個別に扱う場合は追加APIの`run_with_diagnostics(prepared, &Path)`を使用してください。エラーの`RunFailure`は最後の`diagnostic`、先行する`prior_diagnostics`、結果公開前の`original_termination`を保持します。[公開APIと実装境界](docs/アーキテクチャ設計書.md#公開apiと互換性)を参照してください。

### 結果ビューア

結果を埋め込んだ単独HTMLを作成し、ブラウザで時系列に沿って確認できます。

```bash
./target/release/dir-simulator view \
  --input results/results.json \
  --output results-viewer.html
```

`results-viewer.html`をブラウザで開いてください。WSLからWindowsの既定ブラウザで開く場合は、次を実行できます。

```bash
explorer.exe "$(wslpath -w "$PWD/results-viewer.html")"
```

サーバー起動や追加パッケージのインストールは不要です。出力HTMLは新規ファイルを指定します。シミュレーションの結果ファイルとmanifestは変更しません。HTMLには元の結果と入力スナップショットも含まれるため、別の環境へ渡す場合も同じ記録を表示できます。

| 操作 | 表示・動作 |
| --- | --- |
| 再生・一時停止／速度変更 | 観測期間を再生。1×では全期間を約8秒で表示 |
| スライダー／時刻指定／前後のイベント時刻 | 任意時刻へ移動。ps単位の時刻を整数精度で保持 |
| ノード・バス構成図 | 正確なController–Bus接続とGW内のController間論理経路を描画。ステップ操作では送信をController→Bus、受信をBus→Controllerへのメッセージ移動で表示し、連続再生では送受信の線を強調。Gatewayは所属Controllerを囲む大きなブロックで表示 |
| タイムライン／拡大・縮小 | ノード別TX/RX、バス占有・間隔、送信待ち、破棄を表示 |
| ノード状態と件数 | 選択時刻までの生成・成功・破棄・受信とキュー数を表示 |
| 要求の選択／検索／状態フィルタ | 生成・SOF・EOF・解放・受信時刻と元レコードを確認 |
| Gateway転送／コピー要求の選択 | GW処理・TX処理・TX受理待ち、RX保持件数/容量と親要求、コピー生成・hop破棄・RX破棄・経路不一致、origin/parentと終端への経路遅延を確認 |
| 「結果ファイルを開く」／ドロップ | 別の`results.json`に切り替え |

[ビューア本体](crates/dir-simulator/src/tool/viewer/assets/index.html)を直接ブラウザで開き、JSONを選択する使い方も可能です。この場合は同じフォルダのCSS/JavaScript一式が必要です。どちらの方法もファイルをブラウザ内で処理し、外部通信は行いません。

同時刻は、その時刻の確定済み記録をすべて反映した状態を表示します。前後ボタンは異なる記録時刻へ移動し、スケジューラ内部の同時刻イベント順を再現するものではありません。状態は到達時刻から復元し、予定EOF・予定解放を完了として扱いません。タイムラインと詳細時刻は実行全体の記録、件数と状態は選択中の時刻に対応します。部分結果には「部分結果」を表示します。

前後のイベント時刻ボタンと左右キーでは、移動先の直前の記録時刻から移動先までを再表示します。同じ時刻への移動は、進む場合も戻る場合も同じ動きになります。最初の記録時刻には再表示する区間がありません。その区間の送信をControllerからBusへ約0.7秒で移動させ、その完了後に受信をBusからControllerへ約0.7秒で移動させます。CANの送受信のみを示す場合は合計約1.4秒です。受信だけの区間は受信の移動から始まります。Gateway内も入口Controller→内部転送点を青の送信、内部転送点→出口Controllerを橙の受信で表示します。内部転送点は論理経路の中央です。メッセージと矢印は同じ折れ線上に置き、分岐先ごとに経路を分けます。ステップは親CANの受信、Gateway内の送信・受信、出口CANの送信・受信の順に再表示し、出口TX受理が確定していない枝には内部受信を追加しません。TX容量待機は入口に表示します。前の時刻へ戻る場合も通信方向は同じです。遅延ゼロの同報受信も各受信先へ表示します。アニメーション中の選択時刻・件数は移動先の確定状態で、物理的な伝搬時間の再現ではありません。連続再生ではメッセージを動かさず、CAN接続とGateway内の経路について、送信線を青、受信線を橙、通常の接続を灰で表示し、強調線に通信方向の矢印を付けます。短い通信を確認できるよう、直近の完了も観測期間の1/40だけ線を強調します。時刻指定・スライダーで停止位置を選んだ場合は、その時刻の論理進捗と直近の完了印を表示します。受信完了の件数・時刻は確定記録だけで更新します。構成図の接続はmetadata.topology.controllersのBus所属とTX/RX経路遅延を使い、通信記録のないControllerも正確に表示します。古い結果でtopologyがない場合は要求・受信記録から復元し、記録のないノードの接続を推定として区別します。Gatewayの所属はschema2の正規化設定から取得し、通信していないControllerもブロック内に表示します。狭い画面では内部のControllerを縦に並べます。

タイムラインの描画は500区間、要求一覧は1ページ75行を上限とし、省略件数を明示します。Gateway転送は受信時刻順の最新500行を表示します。多い場合は検索・状態フィルタ・拡大を利用してください。構成図の送受信アニメーションと状態集計はこの描画上限や検索条件に依存せず、全要求を対象とします。対応入力はschema 1の`can.cc.ideal.v1`とschema 2の`can.cc.multibus.v1`です。元要求から終端までの経路遅延は、GW portを除く終端Controllerの確定received時刻と元要求generated時刻の差です。

ビューアの時刻・状態復元テストはNode.jsで実行できます。実GW結果を生成する試験が`target/debug/dir-simulator`を呼ぶため、先にデバッグ版をビルドしてください。ブラウザ本体にはNode.jsは不要です。

```bash
cargo build --locked -p dir-simulator
node --test tests/viewer_model.test.cjs
```

ブラウザ操作の自動確認は任意で、Playwrightをテスト用に配置して実行できます。

```bash
cargo build --locked -p dir-simulator
npm install --prefix tmp/viewer-tests playwright@1.62.1
./tmp/viewer-tests/node_modules/.bin/playwright install chromium
NODE_PATH="$PWD/tmp/viewer-tests/node_modules" node tests/viewer_browser.cjs
```

### 対応範囲と検証

標準11bit／拡張29bitのClassical CANデータフレーム、CRC-15、内容依存ビットスタッフィング、優先度仲裁、理想ACK、同報、有限キュー、受信フィルタ、固定伝搬・処理遅延、明示列／周期負荷を実装しています。`can.cc.ideal.v1`は単一バス、`can.cc.multibus.v1`は1個以上の独立バスと任意のGWを扱い、各バスに2個以上のControllerが必要です。NEDは宣言・スカラーポート・接続・compound展開の対応サブセットを読み込みます。

この版では、汎用Registry/Envelope拡張API、診断の完全な構造化位置情報、全宣言・値の採用元を含む再現メタデータは未実装です。準備失敗はCLIのE-0001診断で停止し、schema2の準備失敗結果は生成しません。メタデータの不足とGW拡張APIの境界は`metadata.implementation_coverage`にも記録します。台帳・観測行・出力はメモリに保持するため、100万要求の性能・メモリ目標は未検証です。CANエラー状態・再送、CAN FD、他プロトコルは将来対象です。

```bash
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

自動試験は、既存8シナリオ、9ビットベクトル、GWのfixture、RX/TX容量・受理待ち・分岐、入力拒否、境界時刻、異常停止の状態保持、集計の保存則、CSV/JSON一致、manifestハッシュ、出力上書き防止を確認します。GWの追加試験は入力並べ替え、Ready順、multicastの原子性、出典保持を含みます。ソースは[crates/dir-simulator](crates/dir-simulator)、実行例は[examples/can](examples/can)と[examples/gateway](examples/gateway)です。

### Gatewayと複数CAN BUSを実行する

[fanout.ini](examples/gateway/fanout.ini)は、500kbpsのBUS-AからGatewayを経由して250kbpsのBUS-Bと1MbpsのBUS-Cへ分岐します。BUS-Bには別の周期負荷を与え、バスごとの送信時間と待ちを比較できます。

10msの実行ではnative 30要求から20コピーを生成し、コピーは全件成功します。BUS-Bの高負荷によりnative要求は1件送信中・9件待機で終了します。占有率はA=24.4%、B=97.52%、C=12.2%で、独立したバス状態を比較できます。[確認結果と実行ログ](docs/verification/results/gateway-2026-10-03.json)に対象ソースのhashと検証環境を保存しています。

```bash
./target/release/dir-simulator validate --config examples/gateway/fanout.ini
./target/release/dir-simulator run --config examples/gateway/fanout.ini --output results-gateway
./target/release/dir-simulator view --input results-gateway/results.json --output gateway-viewer.html
```

RX保持を確認する[buffered-fanout.ini](examples/gateway/buffered-fanout.ini)は、各ingressのRX容量2、BUS-B/CのTX容量1、BUS-A=500kbps・BUS-B=125kbps・BUS-C=1Mbpsで、110us間隔の10フレームを2ms実行します。ビューアを450usへ移動するとMain.gw.aのRX保持2/容量2と、GW内の論理経路2本を確認できます。結果はnative10件・コピー12件、RX解放6件・RX満杯破棄4件、RX最大2です。[改訂後の検証記録](docs/verification/results/gateway-rx-buffer-2026-10-03.json)にソースhashと試験範囲を保存しています。

```bash
./target/release/dir-simulator run --config examples/gateway/buffered-fanout.ini --output results-buffered-gateway
./target/release/dir-simulator view --input results-buffered-gateway/results.json --output buffered-gateway-viewer.html
```

INIで`model-profile = "can.cc.multibus.v1"`と`model-config = "routing.json"`を指定します。NEDの`dir.can.MultibusController`と`dir.can.MultibusBus`を使用し、routingでGW moduleのportとCAN ID範囲を定義します。経路範囲の重複、同じバスのID所有者競合、同一format/IDの循環は準備時に拒否します。GW所属portのnative generatorも拒否します。

転送はingressの受信完了時に有限RX枠へ受理し、GW処理遅延、egressのTX処理・有限キュー・仲裁を順に通ります。routingのGatewayに`rx_queue_capacity`（既定64、整数0～u32最大値）を指定すると、各ingressに同じ容量を個別に適用します。TXが満杯ならRXで空きを待ち、全出力枝がTXへ受理又は終端した時にRXを解放します。空いている出力枝は独立に進みます。RX満杯は新着を`rx_queue_full`、TX容量0はコピーを`queue_full`で破棄します。同一フレームを複製し、origin/parent/hopで経路を追跡し、元バスの成功と受信完了を保持します。GWを使わず、`gateways: []`で独立バスだけを実行することもできます。

結果JSONとmanifest・CSVはschema2になり、`simulation.model_records`へ`can.request`、`can.receiver`、`gw.forward`、`gw.rx_buffer`を格納します。CLI応答と診断は従来のschema1です。バス別の受信数・遅延・占有率、GWのコピー件数・RX保持長/最大・RX破棄・TX受理待ち時間を出力します。schema2のRequestでは`ready_ps`がTX処理完了、`model_fields.tx_enqueued_ps`が実際のTX受理時刻です。ビューアはRX保持とTX受理待ちを選択時刻から復元し、巻き戻しにも対応します。入力・転送・出力の契約は[GW仕様](docs/specs/models/GWモデル詳細機能仕様書.md)、試験範囲は[GW検証仕様](docs/verification/cases/GWモデル検証仕様書.md)を参照してください。

CAN/GWの動作はRust製品試験と独立した静的期待値の照合、ビューアはNode・ブラウザ試験で検証します。OMNeT++を使用した実行結果・比較記録とその添付成果物は掲載対象から除外しています。

[過負荷例](examples/can/overload.ini)と[baseline例](examples/can/baseline.ini)の結果は、上記の`run --config`と`view --input`へ各INI・生成したresults.jsonを指定して再現できます。

## 3層アーキテクチャ

現行コードの[ソース配置と公開API](docs/アーキテクチャ設計書.md#source-layout)では、`lib.rs`を公開入口、`run.rs`を実行統括、`lib/`を共通・モデル別の入力／結果型、`input/`・`runtime/`・`output/`を各処理層、`tool/viewer/`をビューア資産と試験へ分けています。Rust利用側は`PreparedSimulation`と`Snapshot`の`common`・`can`・`gateway`を参照します。CLIおよびJSON／CSVの契約は維持します。

![DIRの3層アーキテクチャ](docs/diagrams/readme/readme--definition-initialization-runtime--component.svg)

[図のソース](docs/diagrams/readme/readme--definition-initialization-runtime--component.puml)

### Definition（定義）

NEDでシステム構造、モジュール型、ポート / ゲート、接続、デフォルトパラメータを記述します。

### Initialization（初期化）

INIファイルでシミュレーションシナリオを記述し、インスタンスのパラメータを上書きします。

### Runtime（実行）

Rustモジュールで振る舞いを実装します。Module Registry（モジュールレジストリ）が、振る舞いを持つNEDのsimple module型とRust実装を対応付けます。compound moduleは子モジュールと接続に展開します。

ノードは必要なケイパビリティ（送受信、バッファリング、転送、調停など）を組み合わせて構成し、接続可否はポート単位で検証します。実行結果は時系列・集計値として記録し、CSVとJSONで出力します。

## 互換性の方針

| 項目 | 方針・範囲注釈 |
| --- | --- |
| NED | 構造記述の定義済みサブセットを採用 |
| INI | DIR独自設定を定義。注：omnetpp.ini完全互換は対象外 |
| 実行 | Rustネイティブの登録契約を採用。注：OMNeT++ C++ API、.msg、INETバイナリ/API互換は対象外 |
| 出力 | CSV・JSONを採用。注：Parquet、.vec、.scaは初期対象外 |
| 将来の対応付け | INET/NEDとの対応付けは独立した追加仕様で扱う |

NEDの構文対応と、INIによるパラメータの値解決は区別します。DIRではNED型の共通のデフォルト値をINIでインスタンスごとに上書きします。対応構文とOMNeT++との意味上の差分は要件定義で管理します。

DIRは、OMNeT++の実装コードを移植するのではなく、公開ドキュメントに記載された言語の振る舞いに基づいて独立して実装する方針です。

## ドキュメント

- [要件定義書](docs/要件定義書.md)：固定ID付き要件、受け入れ条件、未確定事項
- [要件階層（別紙HTML）](docs/要件階層.html)：全有効要件の主親・階層上の位置・分解状態（自動生成）
- [機能仕様書](docs/機能仕様書.md)：固定機能ID、入出力、正常時・境界・異常時の振る舞い、全要件から機能への対応と充足確認
- [要件トレーサビリティ一覧](docs/要件トレーサビリティ一覧.md)：要件ごとの経路を1セル1IDで横に並べた対応表と未接続項目（自動生成）。[背景色付きHTML版](docs/要件トレーサビリティ一覧.html)はローカルのブラウザで開けます。
- [アーキテクチャ設計書](docs/アーキテクチャ設計書.md)：責務分担、内部モデル、API案
- [トレーサビリティ管理規約](docs/トレーサビリティ管理規約.md)：6工程の対応記録、全経路検査、変更影響の逆引き
- [ドキュメント作成・運用規約](docs/ドキュメント作成・運用規約.md)：文書体系、要件分解・詳細化・検証の運用、記述テンプレート、文書と図の命名・配置


| 詳細仕様・方針 | 内容 |
| --- | --- |
| [NED詳細機能仕様書](docs/specs/NED詳細機能仕様書.md) | 構造・型・接続 |
| [設定詳細機能仕様書](docs/specs/設定詳細機能仕様書.md) | INI・値・単位 |
| [実行詳細機能仕様書](docs/specs/実行詳細機能仕様書.md) | 時刻・順序・終了 |
| [資源詳細機能仕様書](docs/specs/資源詳細機能仕様書.md) | 容量・占有・遅延 |
| [結果詳細機能仕様書](docs/specs/結果詳細機能仕様書.md) | 計測・集計・出力 |
| [診断詳細機能仕様書](docs/specs/診断詳細機能仕様書.md) | 失敗分類・部分結果 |
| [インタフェース詳細機能仕様書](docs/specs/インタフェース詳細機能仕様書.md) | CLI・登録・拡張 |
| [CANモデル詳細機能仕様書](docs/specs/models/CANモデル詳細機能仕様書.md) | 仲裁・CRC・負荷 |
| [品質・配布方針](docs/品質・配布方針.md) | 性能・精度・出自管理 |
| [将来拡張計画](docs/将来拡張計画.md) | v1.0.0のGW・複数バスと他プロトコルへの境界 |

16分野の詳細機能仕様、入力・結果処理を含む12詳細設計と利用フロー・品質を含む13検証仕様、各モデルの入力fixture・独立期待値を作成しました。Classical CANとGW・複数CANバスの実装と自動試験を追加しました。他モデルの製品実装・実行試験、および仕様全体への対応は今後の作業です。文書の正式名とファイル名をそろえ、自動生成レポートを除く各プロジェクト文書の冒頭に更新履歴を記載し、内容差分はGitで管理します。文書バージョンと更新履歴は、[push時の運用](docs/ドキュメント作成・運用規約.md#document-version-at-push)に従い、前回push以降の変更を文書ごとに一改訂へまとめ、push準備時に一度更新してコミットします。

図の編集元（`.puml`）と表示用SVGは `docs/diagrams/<文書ID>/` に保存しています。ローカルのPlantUMLで全図を更新・確認できます。

```bash
python3 scripts/render_diagrams.py
python3 scripts/render_diagrams.py --check
```

要件から検証までの対応は次で点検できます。通常検査は未完了を報告し、`--strict` は経路に不足があれば失敗します。全193要件を検証仕様まで接続し、構造エラー・未完了項目は0件です。これは文書の追跡経路の検査であり、全要件の製品実装・試験合格を意味しません。

```bash
python3 scripts/check_traceability.py
python3 scripts/check_traceability.py --strict
python3 scripts/check_traceability.py --impact DIR-FUNC-0008
```

トレーサビリティ一覧のMarkdown版・HTML版と要件階層の別紙HTMLは `python3 scripts/generate_traceability.py` で同時に更新できます。階層の編集元は `docs/要件階層.json` です。`--check` で3ファイルの鮮度と階層データの整合性を確認できます。HTML版は薄い背景色で工程と未割当を区別します。Markdown表示側が装飾を除去する場合も、ブラウザでHTML版を開けば背景色を確認できます。コミット時に3ファイルを自動更新する場合は、初回のみ `git config core.hooksPath .githooks` を実行します。

## 開発状況

0.1.0（CAN動作確認版）のRust CLI・ライブラリにGW・複数CANを追加しました。既存の解析期待値によるCAN 8シナリオ、9ビットベクトルとGW 16 fixtureの先行照合は、RX保持・TX満杯待ち導入前の証跡です。改訂したRX/TX契約の期待と実行範囲は[GW検証仕様](docs/verification/cases/GWモデル検証仕様書.md)で区別して記録します。仕様全体への適合完了や、実機CANとの適合認証を示すものではありません。
CANの再現範囲、NED/INIの詳細、時間と終了条件、バッファ・調停、計測定義、実行環境を詳細仕様に規定しました。要件193件、機能48件、受け入れ条件52件、TBD台帳15件（全件解決済み）を表形式で管理しています。番号付きIDは`DIR-REQ-0001`のように4桁です。

## ライセンス

DIR Simulatorのプロジェクト作成物は、別途ライセンス表示がある場合を除き、MIT LicenseまたはApache License 2.0のいずれか（利用者が選択）で利用できます。対象にはソースコード、文書、テンプレート、PlantUMLソース、生成図を含みます。ライセンス本文は [`LICENSE-MIT`](LICENSE-MIT) と [`LICENSE-APACHE`](LICENSE-APACHE)、適用範囲は [`LICENSE`](LICENSE) を参照してください。

第三者の素材・依存関係はこの許諾の対象外で、それぞれのライセンス条件に従います。配布条件と出自の確認方法は[品質・配布方針](docs/品質・配布方針.md)に規定しています。

## CAN実装の入口

| 資料 | 内容 |
| --- | --- |
| [CAN詳細機能仕様](docs/specs/models/CANモデル詳細機能仕様書.md) | 入出力・bit計算・時刻・仲裁・状態の外部契約 |
| [CAN詳細設計](docs/design/CANモデル詳細設計書.md) | BusContext、codec、シリアライズ、状態遷移と処理手順 |
| [共通実行詳細設計](docs/design/実行詳細設計書.md) | ヒープ・dirty集合・EffectBatch・確定journal・登録 |
| [CAN検証仕様](docs/verification/cases/CANモデル検証仕様書.md) | 6ケース、NED/INI/workload、bitvectorとシナリオ期待値 |
| [共通実行検証仕様](docs/verification/cases/共通実行検証仕様書.md) | 4ケース、時刻・順序・失敗・拡張 |

仕様に同梱したCAN bitvectorの照合は次で実行できます。これは期待値の確認であり、製品シミュレータの動作試験とは区別します。

```bash
python3 docs/verification/fixtures/can/verify_vectors.py
```

## 追加モデル実装の入口

| モデル | 機能仕様 | 詳細設計 | 検証仕様 |
| --- | --- | --- | --- |
| GW | [仕様](docs/specs/models/GWモデル詳細機能仕様書.md) | [設計](docs/design/GWモデル詳細設計書.md) | [検証](docs/verification/cases/GWモデル検証仕様書.md) |
| AXI | [仕様](docs/specs/models/AXIモデル詳細機能仕様書.md) | [設計](docs/design/AXIモデル詳細設計書.md) | [検証](docs/verification/cases/AXIモデル検証仕様書.md) |
| Ethernet | [仕様](docs/specs/models/Ethernetモデル詳細機能仕様書.md) | [設計](docs/design/Ethernetモデル詳細設計書.md) | [検証](docs/verification/cases/Ethernetモデル検証仕様書.md) |
| profile統合 | [仕様](docs/specs/拡張モデル共通詳細機能仕様書.md) | [設計](docs/design/複数モデル統合詳細設計書.md) | [検証](docs/verification/cases/拡張モデル統合検証仕様書.md) |

| Ethernet媒体 | 追加した評価範囲 |
| --- | --- |
| 10/100Mbps半二重 | CSMA/CDのcarrier sense、衝突、jam、backoff、再試行上限、送信待ちを評価 |
| 1000BASE-T1 | 全二重1Gbps、MASTER/SLAVEの構成、両端PHY固定遅延を評価。注：半二重はT1の動作モードには含まれない |
| v1互換 | 既存Ethernetの全二重profileを維持し、媒体別設定はethernet.l2.store-forward.v2で選択 |
| 実装資料 | [媒体詳細仕様](docs/specs/models/Ethernet媒体拡張詳細機能仕様書.md)、[詳細設計](docs/design/Ethernet媒体拡張詳細設計書.md)、[検証仕様](docs/verification/cases/Ethernet媒体拡張検証仕様書.md) |

## 当初対象の網羅

当初の15対象と追加した10BASE-T・1000BASE-T1は、0.1.0のClassical CANと[将来対応予定の16対象](docs/要件定義書.md#original-target-coverage)に分けて管理し、将来対応予定表で有効要件へ対応付けています。対象範囲と段階的な実装順序を分け、CAN FD・100BASE-T1・SoC／AHB／NoC・DDR／SRAM・共有メモリIPC／DMA／メールボックスIPCも要件から検証ケースまで規定します。

| 対象 | 仕様 | 設計 | 検証 |
| --- | --- | --- | --- |
| CAN FD・100BASE-T1 | [詳細仕様](docs/specs/models/CANFD・100BASE-T1詳細機能仕様書.md) | [詳細設計](docs/design/CANFD・100BASE-T1詳細設計書.md) | [検証仕様](docs/verification/cases/CANFD・100BASE-T1検証仕様書.md) |
| SoC・AHB・NoC | [詳細仕様](docs/specs/models/SoC・AHB・NoC詳細機能仕様書.md) | [詳細設計](docs/design/SoC・AHB・NoC詳細設計書.md) | [検証仕様](docs/verification/cases/SoC・AHB・NoC検証仕様書.md) |
| DDR・SRAM・共有メモリIPC・DMA・メールボックスIPC | [詳細仕様](docs/specs/models/メモリ・IPC詳細機能仕様書.md) | [詳細設計](docs/design/メモリ・IPC詳細設計書.md) | [検証仕様](docs/verification/cases/メモリ・IPC検証仕様書.md) |

CAN FDの初期profileは、証跡付きの位相別bit数を入力する時間評価モデルです。Classical CANの内容依存CRC・stuffing計算とは再現範囲を分けて記録します。GW以外の追加モデルの製品実装・実行試験は未実施です。
