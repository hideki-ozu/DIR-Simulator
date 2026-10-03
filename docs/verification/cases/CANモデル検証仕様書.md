# CANモデル検証仕様書

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。bit vector、8実行入力、期待値、状態・異常検証ケースを追加 |

文書ID：`verification-can`

| 項目 | 内容 |
| --- | --- |
| ケース状態 | 検証仕様確定。static fixtureの内部整合を確認済み。シミュレータ実行試験は未実行 |
| 環境 | Python 3標準ライブラリで解析fixtureを確認する。モデル実装後はdir-simulator CLIと結果schema version1を使用する |
| 実入力 | [Main.ned](../fixtures/can/models/demo/Main.ned)、[scenarios.json](../fixtures/can/scenarios.json)が指す8組のINI/workload JSON。Main.a/b/cとMain.bus、500kbps。個別overrideは各INIを正とする |
| 実行手順 | リポジトリrootから`dir-simulator run --config docs/verification/fixtures/can/competition.ini --output /tmp/dir-can-competition`。出力先は事前に存在しない名前を使い、各caseのconfigへ置換して別出力へ保存する。注：CLI実装・実行成功を本書作成で保証した扱いにしない |
| 照合手順 | results.jsonのRequest/Receiverと計測records/summaryを読み、scenarios.jsonのexpected projectionと比較する。D時刻・件数を整数へ変換してから照合。fixtureのidはRequest.request_id、sof_orderはsof_ps昇順と同時刻event順で求める |
| 判定 | 整数bit・ps・件数・状態は完全一致。浮動小数の結果指標は結果仕様の判定規則を適用する。fixtureに記載のないevent sequence絶対値を推測して合格条件へ追加しない（注） |
| 根拠 | [CAN仕様](../../specs/models/CANモデル詳細機能仕様書.md)、[CAN設計](../../design/CANモデル詳細設計書.md)。CRC polynomialは[CiA公開一次資料](https://www.can-cia.org/can-knowledge/cyclic-redundancy-check-crc-in-can-frames)、構成範囲は[CiA CAN CC](https://www.can-cia.org/can-knowledge/can-cc)を確認。独立実装の解析値であり、実機採取・ISO適合検証の証拠は対象外（注） |


<a id="dir-test-0001"></a>

## 1. CRC・stuff・整数時間

```trace
{
  "id": "DIR-TEST-0001",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0034",
    "DIR-REQ-0036",
    "DIR-REQ-0037",
    "DIR-REQ-0098",
    "DIR-REQ-0099",
    "DIR-REQ-0100",
    "DIR-REQ-0109",
    "DIR-REQ-0110",
    "DIR-REQ-0114"
  ],
  "upstream": [
    "design-can#serialization"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [serialization](../../design/CANモデル詳細設計書.md#serialization)のfield順、CRC、stuff位置、時間丸め |
| 上流 | DIR-FUNC-0009/0015/0023、[DIR-AC-0004](../../要件定義書.md#dir-ac-0004)、[DIR-AC-0018](../../要件定義書.md#dir-ac-0018)、[DIR-AC-0021](../../要件定義書.md#dir-ac-0021)、[wire-time](../../specs/models/CANモデル詳細機能仕様書.md#wire-time) |
| 入力 | [vectors.json](../fixtures/can/vectors.json)の9件。標準0/1/0x123/上限、拡張0/上限、0/8byte、00/ff/交互payload、CRC最終bit直後stuffのID9を含む |
| 手順 | `python3 docs/verification/fixtures/can/verify_vectors.py`を実行。実装後はserialize APIにも同じ入力を渡し全field/bit列を比較する。bitrate=500000,1000000,333333bpsの3値でSOF原点のEOF/releaseを計算する |
| 期待 | v00はCRC0000/S6/frame50/占有53、v01はCRC2213/S3/frame47/占有50。全値は固定JSONに保存。v0000・333333bpsではEOF=150000151ps、release=159000160ps。CRC入力＋CRCの多項式余り=0、destuffで元bit列と一致 |
| 独立性 | 固定vector作成の整数GF(2)長除算とverify側のレジスタを照合。verify側は別のfield列組立、stuff runカウンタと送信prefixを読むdestuffを照合。製品serializeを期待値の生成元に使う処理は対象外（注） |
| 実行結果 | 解析fixture照合は9件合格。製品serialize・時間計算試験は未実行。自動コマンド標準出力を保存して再確認できる |

<a id="dir-test-0002"></a>

## 2. 候補集合・容量・仲裁

```trace
{
  "id": "DIR-TEST-0002",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0035",
    "DIR-REQ-0038",
    "DIR-REQ-0039",
    "DIR-REQ-0040",
    "DIR-REQ-0041",
    "DIR-REQ-0045",
    "DIR-REQ-0046",
    "DIR-REQ-0104",
    "DIR-REQ-0105",
    "DIR-REQ-0106",
    "DIR-REQ-0107",
    "DIR-REQ-0108"
  ],
  "upstream": [
    "design-can#state-machine"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [state-machine](../../design/CANモデル詳細設計書.md#state-machine)のqueue所有権・phase境界・SOF batch |
| 上流 | DIR-FUNC-0012/0016/0017/0023、[DIR-AC-0005](../../要件定義書.md#dir-ac-0005)、[DIR-AC-0007](../../要件定義書.md#dir-ac-0007)、[DIR-AC-0020](../../要件定義書.md#dir-ac-0020)、[arbitration](../../specs/models/CANモデル詳細機能仕様書.md#arbitration) |
| 入力 | competition.ini、queue-full.ini、capacity-zero.ini、release-arrival.ini。個々のworkloadは同名JSON |
| 期待 | competition: a:0 SOF0/EOF100us/release106us、b:0 SOF106us/EOF200us/release206us。queue-full: a:0のみ送信、a:1は同phase1に満杯破棄。capacity-zero: attempt0/drop1。release-arrival: b:0、a:0、b:1の順にSOF0/100/206us |
| 追加入力・期待 | 単一sourceの同時要求ID10/ID2はID2先。同じsource同IDはgenerator ID/ordinal順。standard0x123とextended0x048C0000はstandard先、extended0x123とstandard0x123はextended先。ID1送信中50usにID0到着ならID1を100us解放まで維持 |
| 手順・判定 | 各fixtureを実行してexpectedを照合。追加ケースはcompetition JSONのframe/source/timesを明示値へ置換しgenerator IDを一意にする。入力ファイルのgenerator記述順と初期モデル登録順を逆順にしても同じ要求・時刻・候補を確認。SOFでqueueから一件だけ減り、敗者がpendingのまま保持されることを内部journalレビューで確認 |
| 実行結果 | 3シナリオの時刻projectionは解析照合済み。モデルの仲裁・キュー処理試験は未実行 |

<a id="dir-test-0003"></a>

## 3. 同報・遅延・ACK・payload

```trace
{
  "id": "DIR-TEST-0003",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0033",
    "DIR-REQ-0097",
    "DIR-REQ-0099",
    "DIR-REQ-0103",
    "DIR-REQ-0111",
    "DIR-REQ-0112",
    "DIR-REQ-0113",
    "DIR-REQ-0114"
  ],
  "upstream": [
    "design-can#payloads",
    "design-can#state-machine"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [payloads](../../design/CANモデル詳細設計書.md#payloads)と[EOF/Receiver遷移](../../design/CANモデル詳細設計書.md#state-machine) |
| 上流 | DIR-FUNC-0013/0018/0023、[DIR-AC-0003](../../要件定義書.md#dir-ac-0003)、[DIR-AC-0022](../../要件定義書.md#dir-ac-0022)、[controller](../../specs/models/CANモデル詳細機能仕様書.md#controller) |
| 入力 | competition.iniとdelay-filter.ini。a empty ID0、tx処理3us、a tx経路2us、b rx経路3us、b rx処理7us、c filter none |
| 期待 | delay-filter: SOF3us/EOF103us/release109us。b observed108us/received115us、c observed105us/filtered。a success1、自己Receiver0件、b received1、c filtered1。competitionの送信2件から受信4件。各受信frameは送信frameと完全一致 |
| 追加入力・期待 | bとcを共にfilter noneへ変更しても送信success1/received0/filtered2。b filter std:0x0はID0適合、ext:0x0は不適合。dataを00Ffへ変更してpayload data_hexが00ffへ正規化し同報のコピー全てで一致 |
| 手順・判定 | Receiverの作成はEOF batchと同一commitにだけ現れることをjournalで検査。codec serialize/decode往復を確認。未知キー・重複キーをcodecへ注入しE-0002/invalid_event、復号済み通知のreceiver/target不一致・ready/Envelope時刻不一致をhandlerへ注入しE-0002/model_failedと未commitを確認 |
| 実行結果 | delay-filterの時刻加算を解析照合済み。通知配送・filter・codec実装試験は未実行 |

<a id="dir-test-0004"></a>

## 4. 生成器と準備検証

```trace
{
  "id": "DIR-TEST-0004",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0006",
    "DIR-REQ-0007",
    "DIR-REQ-0024",
    "DIR-REQ-0045",
    "DIR-REQ-0097",
    "DIR-REQ-0098",
    "DIR-REQ-0099",
    "DIR-REQ-0100",
    "DIR-REQ-0101",
    "DIR-REQ-0102",
    "DIR-REQ-0103",
    "DIR-REQ-0114"
  ],
  "upstream": [
    "design-can#workload-validation",
    "design-can#state-machine"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [workload-validation](../../design/CANモデル詳細設計書.md#workload-validation)のcursor・所有者検査・型検証 |
| 上流 | DIR-FUNC-0009/0021/0023/0027、[DIR-AC-0018](../../要件定義書.md#dir-ac-0018)、[DIR-AC-0019](../../要件定義書.md#dir-ac-0019)、[workload](../../specs/models/CANモデル詳細機能仕様書.md#workload) |
| 入力・期待 | mixed-generators.ini: 32usの半開区間でa:0,b:0,a:1,b:1,a:2を2,2,12,12,22usに生成。b:2の32usは未生成。in_flight1/pending4/success0、time_limit。count0、times空、end=startでは該当generatorの生成0 |
| 拒否入力 | 同じ完全入力を複製して一箇所だけ変更：standard ID2048、extended ID536870912、id=true/1.0、data奇数/9byte/空白、dlc追加、period0、phase=period、times降順、重複generator ID、未知kind、別source同format/id、Controller1個、profile未知、bitrate0。空generator又はcount0の所有者重複も同じ拒否 |
| 手順・判定 | 各正常・異常ペアをprepareまで実行。正常は確定frame/cursorを比較、異常はprep_failed/exit2/E-0001と該当フィールド・入力位置を確認しevent callback0件。schema missing/unknown/duplicate keyをJSON各階層で注入して同じ準備境界を確認 |
| 再現性 | generator記述順を反転して生成ID・時刻が同じことを比較。end/count/Tの先にある要素も型検査されることを確認 |
| 実行結果 | 入力JSONの構文と参照先8組は静的確認済み。DIR parser/dispatcher実装試験は未実行 |

<a id="dir-test-0005"></a>

## 5. 停止・原子性・所有権

```trace
{
  "id": "DIR-TEST-0005",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0103",
    "DIR-REQ-0115",
    "DIR-REQ-0120"
  ],
  "upstream": [
    "design-can#state-machine"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [state-machine](../../design/CANモデル詳細設計書.md#state-machine)の失敗prefix、要求数保存、所有者解放 |
| 上流 | DIR-FUNC-0014/0018/0019/0020/0023、[DIR-AC-0024](../../要件定義書.md#dir-ac-0024)、[controller](../../specs/models/CANモデル詳細機能仕様書.md#controller) |
| 入力・期待 | eof-boundary.iniのT100us: generated1/in_flight1/success0/Receiver0/bus transmitting。after-eof.iniのT100us+1ps: success1/received2/bus intermission/release_ps null。queue-fullではgenerated2=success1+dropped1 |
| 追加境界 | delay-filterのT108usならb pending/observed null、T115usならb pending/observed108us/received null。SOF以前を打切るTX処理T3usならprocessing1/ready null。全fixtureでgenerated=success+dropped+processing+pending+in_flightを完全一致させる |
| 注入・手順 | テスト用ContextでSOFの2件目予約検証、EOF同報の最後の予約検証を失敗させ、直前commitのjournalを保存する。SOF失敗ならpending/attempt0/予定時刻null、EOF失敗ならin_flight/success0/Receiver0を確認。二重EOF/token不一致/unknown requestも個別に注入 |
| 内部判定 | 失敗batchのRequest・Receiver・queue・busy計測・future eventの部分公開0件、execution_failed、failed eventが未処理集合に1件保持されること。finishは1回、snapshot保持、pending eventを実行せずメモリを解放することをレビュー。注：OS abortなど制御不能停止の保証は診断仕様の対象外境界に従う |
| 実行結果 | 境界の期待値を仕様から算出。失敗注入・資源解放・統合試験は未実行 |

<a id="dir-test-0006"></a>

## 6. プロファイル統合と拡張境界

```trace
{
  "id": "DIR-TEST-0006",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0001",
    "DIR-REQ-0058",
    "DIR-REQ-0097",
    "DIR-REQ-0114",
    "DIR-REQ-0116"
  ],
  "upstream": [
    "design-can#workload-validation",
    "design-can#state-machine"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [workload-validation](../../design/CANモデル詳細設計書.md#workload-validation)とBusContextのprofile隔離 |
| 上流 | DIR-FUNC-0023/0027、[DIR-AC-0009](../../要件定義書.md#dir-ac-0009)、[DIR-AC-0023](../../要件定義書.md#dir-ac-0023)、[DIR-AC-0025](../../要件定義書.md#dir-ac-0025)、[profile](../../specs/models/CANモデル詳細機能仕様書.md#profile) |
| 入力・期待 | 8個のstatic scenarioを全件実行し非競合・競合・容量飽和・停止を組み合わせて照合。metadataでprofile can.cc.ideal.v1、確定値、初期state、全入力snapshotと実装識別を確認する |
| 境界レビュー | CAN固有CRC/format/idが共通scheduler比較キーに入らず、型codec・優先規則・BusContextへ閉じること。別BusContextをunit testで二個構築し一方の状態更新が他方へ影響0であること。現profileの二busネットワーク入力は準備失敗として維持する |
| 追加拒否 | remote/FD/XL/error injection/retry/TEC/REC/passive/bus-off/seed/distribution設定を各1件与え未対応入力として準備失敗。ACKなし成功の合成や未知profileのfallbackがないことを確認する |
| 判定範囲 | 本caseは小規模のCAN連携とprofile境界を担当する。[品質・配布方針](../../品質・配布方針.md)の中規模32ノード性能試験は[DIR-TEST-0084](利用フロー・品質検証仕様書.md#dir-test-0084)へ分担する。注：本fixtureで性能要件全体の合格を主張する処理は対象外 |
| 実行結果 | fixture提供済み、製品統合・性能測定・ISO規格適合試験は未実行 |

## 7. 実行記録

| 日付・対象 | 結果・証拠 |
| --- | --- |
| 2026-09-29・本文とstatic fixture | Python標準ライブラリのverify_vectors.pyで9件のbit vector、8組のfixture参照、3シナリオ時刻projectionを照合。CRC二方式・destuff・field組立・受信遅延加算が一致。コマンドのPASSは解析資料の整合を示す |
| 実装対象 | 対象コミットは文書冒頭の未リリース作業ツリー。simulator実行、実測結果、失敗注入証跡は未取得。実装後の実行記録にcommit・環境・config・expected・実測値・合否・結果パスを追加する |
| 参照資料確認 | 2026-09-29にCiA CAN CC/CRC公開資料とBosch CAN Specification 2.0 Part Bの原著者文書（第三者ホスト複製）を参照。ISO11898-1:2024全文の網羅照合は未実施 |

### DIR-TEST-0004の設定と負荷の統合確認

同じ[demo.Main](../fixtures/can/models/demo/Main.ned)を二つのINIで使い、[delay-filter.ini](../fixtures/can/delay-filter.ini)のT=300us・bitrate=500kbps・a.txProcessingDelay=3usを基準とする。変更側はa.txProcessingDelayだけを5usへ変える。全generatorと接続は同一で、request a:0のgeneratedは両方0、ready/SOF/EOF/release/受信時刻は変更側で2us後ろへ移る。b.rxProcessingDelay=7us、c.rxFilter=none、経路delayは保持される。準備で確定した値、factoryへ渡る値、発火後の値、結果metadataを照合し、DIR-REQ-0006・0007・0024の負荷側の分担を確認する。全体の操作・出力と独立時刻値は[DIR-TEST-0080](利用フロー・品質検証仕様書.md#dir-test-0080)を併用する。製品実行は未実施。
