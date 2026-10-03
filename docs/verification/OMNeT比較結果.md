# OMNeT比較結果

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | Gateway・RX追加後の78ケース・156実行と差分、保存証跡、未実施範囲を追加。文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | 43ケースの実行結果、モデル差の妥当性判断、再現用証跡を公開版へ集約 |

文書ID：`verification-omnet-comparison`

文書状態：比較実行済み。異なるモデルの観測結果の比較であり、CAN適合認証ではない。

## 1. 実行条件と結果

2026-10-03（JST）にDIRとOMNeT++ 6.4.0 / FiCo4OMNeTで43ケース、計86実行を完了した。8シナリオ、3負荷例、混在ID・観測境界5ケース、9フレーム×3速度を対象とした。各入力の時刻・ID・形式・実payload・速度・ノード数・観測期間を照合し、生イベントの重複、状態遷移、遅延、同報先、キュー履歴の整合を検査した。

共通観測項目は4ケースで一致、39ケースで差異があった。一致したcapacity-zero、zero-horizon、empty-generators、future-onlyでは実際の送信はない。差異は比較プログラムの実行失敗を意味しない。

| ケース | DIR完了 | FiCo完了 | DIR破棄 | FiCo破棄 |
| --- | ---: | ---: | ---: | ---: |
| baseline | 60 | 60 | 0 | 0 |
| contention | 81 | 90 | 72 | 72 |
| overload | 81 | 90 | 1112 | 1104 |

DIRの完了はEOF、FiCoの完了は3bitのintermissionを含むnative完了であり、別の観測点として記録した。双方の集計窓は半開区間`[0,H)`。有限キュー、drop-tail、TX/RX処理遅延、配送遅延、受信フィルタは外部アダプターでそろえた。FiCoのバス・仲裁・フレーム時間計算は変更していない。詳細は[比較ツール](../../tools/omnet_comparison/README.md)を参照する。

## 2. 差分の妥当性

今回のpayloadに対するフレーム時間と高負荷時の処理件数は、DIRのほうがClassical CANのビット列計算に合致する。FiCoは固定stuff率0の設定であり、内容依存のstuff bitを省いている。CANでは同じ値が5bit続くと反転bitを挿入し、EOF後には3bitのintermissionを設ける。[CiAのClassical CAN解説](https://www.can-cia.org/can-knowledge/can-cc)

高負荷時に連続送信される標準ID`0x100`、payload `0102030405060708`を、DIRのcodecを呼ばずCRC多項式除算とビット列走査で独立計算した。CRCは`0x5409`、stuffは11bitとなる。

| 項目 | 独立計算・DIR | 今回のFiCo |
| --- | ---: | ---: |
| SOFからEOF | 119bit | EOF単独では観測しない |
| intermissionを含む占有長 | 122bit | 111bit |
| 500kbpsでの占有時間 | 244µs | 222µs |
| 20ms以内の完了 | 81件 | 90件 |

DIRの81件目のEOFは19,758µs、82件目は20,002µsとなる。FiCoは初回に1bit時間の待機があり、90件目のnative完了は19,982µs。DIRをintermission後で数えても81件なので、この件数差は観測位置だけでは説明できない。

`release-arrival`では、DIRのバス解放は100µsで、同時刻に高優先度要求がreadyとなる。その仲裁に参加可能な状態まで送信準備ができている前提ならDIRの順序が妥当。FiCoは時間近似により96µsに次フレームを開始しているため、100µsの要求は参加できない。FiCoが仲裁優先順位を逆にしたという意味ではない。

有限キューの方針や同時刻の送信準備期限はコントローラー・ソフトウェアにも依存する。333333bpsでのps端数はDIRの切り上げとFiCoの時刻量子化の差として区別する。DIRも理想ACK・誤りなしのモデルであり、物理同期、伝搬による仲裁・ACKへの影響、エラー・再送まで実機と一致すると判断したものではない。

## 3. 保存した証跡

[全ケースの入力・生結果・集計アーカイブ](results/omnet-v0.1.tar.gz)に、実行時の`report.md`、`report.json`、`cases/`を改変せず保存した。実行ファイル・共有ライブラリ等のビルド出力は含めない。アーカイブSHA-256は次のとおり。

```text
a4b041090f8fb8da071dfcd548fd99ad07c8160340db9146da6918eb20fed4e2
```

`report.json`のprovenanceには実行時の未コミット状態、ソース・バイナリのハッシュ、元の絶対パスを保持している。これは公開前の実行を示す履歴であり、コミット済みのv0.1を後から実行したという記録ではない。アーカイブ内の生成レポートには独立した文書版を付けない。

```bash
mkdir -p tmp/omnet-v0.1-evidence
tar -xzf docs/verification/results/omnet-v0.1.tar.gz -C tmp/omnet-v0.1-evidence
```

再実行は比較ツールの手順を使用する。CRC/bit列適合、DIR固有の入力拒否・内部イベント数・整数上限・出力・ビューア、未実装の将来モデルはOMNeT比較の対象外であり、アーカイブの報告書に理由を記録している。

## 4. Gateway・RX追加後の現行実装比較

2026-10-03（JST）、Gateway/RX実装を含む現在の未コミット作業ツリーで、単一CAN43ケースとGateway35ケースを実行した。合計78ケース・156実行が完了し、入力対応・生イベントの整合検査で実行失敗はなかった。共通観測項目の完全一致は5ケース、差異は73ケースだった。完全一致したケースはいずれも実際の送信がないため、この数字だけで通信モデル全体の一致とは判断しない。先のv0.1比較を上書きした記録ではない。

| 対象 | ケース数 | 件数ベクトル一致 | バスごとのSOF順序一致 | 共通観測項目の完全一致 |
| --- | ---: | ---: | ---: | ---: |
| 単一CAN | 43 | 40 | 40 | 4 |
| Gateway・複数CAN | 35 | 27 | 28 | 1 |
| 合計 | 78 | 67 | 68 | 5 |

Gateway35ケースの内訳は有効fixture13件、Rust試験の入力変形15件、buffered-fanout例1件、追加停止境界6件。invalid-overlap/owner/cycleはDIRの入力拒否試験として扱い、通信実行と数えない。既定RX64、65件目の拒否、u32最大容量、RX0、TX0、Gateway/TX処理中のRX保持、TX満杯からの再開、分岐先の独立進行、hop上限、no_route、受信filter、複数ingressからの待機順を対象にした。Bus gate名変更、route/port/generator配列反転の3組は、各エンジン内の全projectionが一致した。

### 4.1 比較モデルと検査

単一CANと各Gateway接続バスは、インストール済みOMNeT++ 6.4.0 / FiCo4OMNeTのCANバス・ポート・仲裁を実行する。Gatewayのroute、有限RX、TX空き待ち、分岐、hopは独立した外部試験アダプターをOMNeT++のイベント列で実行した。SignalsAndGatewaysの既存Gateway実装そのものを検証した結果ではない。通常CAN ControllerのアプリケーションRXは今回の対象ではない。

各ケースの元INI/NED/workload/routingを固定し、両エンジンの実行前に独立変換した。DIRのSOF・EOF・CRC・stuffing・計画時刻・出力をOMNeT++入力へ使っていない。変換した容量・遅延・速度・route・hop・配線はDIR実行メタデータと照合した。生成・転送・全受信イベントの実payload、ID、形式、origin/parentとhopを検査し、同一バス以外への配送、送信重複、TX待機FIFO違反、容量超過、未完了分岐の欠落、RXの早期／遅延解放を拒否する。バス占有時間とSOF順序はバス単位で保存し、RX/TXキュー積分は整数psで算出する。

停止境界の追加6件は、先に取得したOMNeT++の転送116µs、TX空き待ち再投入480µs、元フレームのnative完了96µsをそれぞれHとH+1psに設定し、同一入力で両方を再実行した。観測時刻の参照元CSVのハッシュも記録した。OMNeT++ではHのイベントを実行せず、H+1psでは実行することを確認した。同時刻の実行順は時刻・priority・投入順で決まるため、最高priorityの停止イベントで半開区間を作る。[OMNeT++公式マニュアル](https://doc.omnetpp.org/omnetpp/manual/#sec:simple-modules:events-and-event-execution-order)

### 4.2 RXと転送の比較結果

以下の件数はDIR / OMNeT++の順。実験ごとにHを固定している。

| ケース | H | 転送コピー | RX保持 | RX解放 | RX破棄 |
| --- | ---: | ---: | ---: | ---: | ---: |
| RX満杯・drop-newest | 1ms | 3 / 3 | 0 / 0 | 3 / 3 | 1 / 1 |
| RX容量0 | 1ms | 0 / 0 | 0 / 0 | 0 / 0 | 3 / 3 |
| 既定RX64・65フレーム | 8ms | 0 / 0 | 64 / 64 | 0 / 0 | 1 / 1 |
| RX u32最大・65フレーム | 8ms | 0 / 0 | 65 / 65 | 0 / 0 | 0 / 0 |
| 分岐の保持中打切り | 500µs | 6 / 6 | 1 / 0 | 2 / 3 | 1 / 1 |
| 分岐の待機解消後 | 1.5ms | 6 / 6 | 0 / 0 | 3 / 3 | 1 / 1 |
| buffered-fanout例 | 2ms | 12 / 12 | 0 / 0 | 6 / 6 | 4 / 4 |

queueでは3件目の親RX受理がDIR320µs / FiCo316µs、子TX受理・親RX解放が524µs / 480µsだった。いずれも先行フレームのSOFで空いたTX待機枠へ投入し、EOFまで待たずにRXを解放した。SOF480µsちょうどの停止では両方ともRXを1件保持し、480µs+1psではOMNeT++のみ解放済みになる。DIRの対応する再開は524µsなので、この差を隠す時刻補正はしていない。

内容依存stuffingを持つDIRのID0・空payloadはEOFまで50bit、占有はintermissionを含め53bit。今回のFiCoはIFS込み47bitの近似で、125kbpsではDIRの424µsに対してnative送信期間376µsとなる。FiCoの仲裁開始待機も転送・RX保持時間に影響する。例えばforward-boundaryではDIRの転送時刻120µsが観測窓外、FiCoの116µsが窓内となり、子コピー数は0 / 1。buffered-fanoutの全Request完了数も20 / 21だった。コピー・破棄数が同じでも、停止時の保持や完了数まで同じとは限らない。

### 4.3 現行比較の証跡と未実施範囲

[現行比較サマリー](results/omnet-current-2026-10-03.json)に全ケース一覧・対応するRust試験・件数・差分概要・検査結果・ハッシュを、[全入力・生結果・比較器ソースのアーカイブ](results/omnet-current-2026-10-03.tar.gz)に`can/`と`gateway/`のreport、caseごとの入力・DIR結果・OMNeT++ CSV/sca/vec/log・比較JSON/CSVを保存した。ビルド済み実行ファイル・共有ライブラリは含めない。アーカイブSHA-256はサマリーに記録する。

インストール済みFiCo共有ライブラリのSHA-256は`2b346ae83efc811efe678f1f6edc5f5410bf2d7b8ee884977363dcbc066dbc49`。このバイナリを実際にロードした。現在のFiCo checkoutはupstreamとの差分を持ち、ソースとバイナリのビルド対応は証明していない。ソース・ヘッダーのハッシュは実行時に存在したファイルの識別用である。既存OMNeT++・FiCoのソース、ライブラリ、設定は変更していない。

DIR独自の入力拒否、公開Registry/Envelope API、内部journal・イベント上限・コミット済みprefix、u64オーバーフロー、CRC/bit列、結果公開・viewer、性能・実機／規格適合に同一の通信試験を割り当ててはいない。CAN FD、Ethernet、AXI、SoC、memory、IPCのfixtureもDIR製品未実装のため今回の実行数に含めない。比較器のログ破損・入力対応に対する回帰試験は別に記録する。
