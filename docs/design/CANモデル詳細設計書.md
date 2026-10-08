# CANモデル詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 2f1e60b`
予定公開版：`v1.1.4`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-08` | CAN完了台帳の安定ハンドル・退避条件・停止時凍結と、逐次窓集計の内部責務を追記 |
| `1.0.0` | `2026-10-03` | CANエンジンとcodecのソース配置、GWとの境界を反映。文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。CANの所有権・ビット構成・型付き通知・状態遷移を確定 |

文書ID：`design-can`

| 項目 | 内容 |
| --- | --- |
| 文書状態 | 設計確定。現行CANの実装・回帰試験を実施。公開拡張APIを含む設計全体への適合は未完了 |
| 担当元 | [architecture#arch-domain-models](../アーキテクチャ設計書.md#arch-domain-models)のsim-can責務を具体化する |
| 外部契約の正本 | [CAN仕様](../specs/models/CANモデル詳細機能仕様書.md)、[実行仕様](../specs/実行詳細機能仕様書.md)、[資源仕様](../specs/資源詳細機能仕様書.md)、[結果仕様](../specs/結果詳細機能仕様書.md)。値域・時刻順序・計測式・終了条件は参照先を適用する |
| 依存 | sim-can→型registry、scheduler効果batch、資源計測。BusContextはsim-can所有とし、共通核は汎用の識別・時刻・payload bytesを扱う |

現行実装ではCAN設定・結果型を`src/lib/types/can.rs`・`src/lib/snapshot/can.rs`へ置き、`src/runtime/can.rs`が生成・仲裁・送受信を実行する。純粋なビット列・CRC・stuff・受信フィルタ・転送時間の計算は`src/runtime/can/protocol.rs`へ分離する。既存の公開`dir_simulator::can`はprotocolの再exportとして維持する。[ソース配置](../アーキテクチャ設計書.md#source-layout)を参照する。

## 1. シリアライズと不変のフレーム

<a id="serialization"></a>

```trace
{
  "id": "design-can#serialization",
  "stage": "design",
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
    "architecture#arch-domain-models"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 設計要素 | 定義 |
| --- | --- |
| 入力／出力 | `serialize(CanFrame)->WireFrame`は副作用なし。入力範囲は[frame-input](../specs/models/CANモデル詳細機能仕様書.md#frame-input)、bitの意味とCRC/stuff計算は[wire-time](../specs/models/CANモデル詳細機能仕様書.md#wire-time)を正本とする |
| WireFrame | `crc_input:bit[]`、`crc15:u16`、`stuffed_region:bit[]`、`stuff_positions:u16[]`、`frame:bit[]`、`frame_bits:u16`、`payload_bits:u8`。stuff_positionsはstuffed_regionの0始まりindex。frameはSOF～EOF。intermissionは別の3bit |
| cache・寿命 | 準備時にframe値ごとに算出し、実行終了まで不変共有する。要求・受信通知は同じframe値を参照する。イベント順序はgenerator/instanceの確定順から求める |
| 算術 | bitの組立は固定長、時間の乗算・加算はchecked u128後にu64値域を検査する。上限違反は実行仕様の時間算術失敗。時間はSOF原点から一度ceilする。注：EOFから丸め済みintermission時間を足す方式は対象外 |
| 計算確認 | [vectors.json](../verification/fixtures/can/vectors.json)の全bit列・stuff位置と照合する。CRCレジスタとGF(2)長除算の独立表現を[verify_vectors.py](../verification/fixtures/can/verify_vectors.py)で比較する |

以下は正本のbit規則を処理順へ写した擬似コード。`bits(n,w)`はw桁・MSB先行、`++`は連結、`ceildiv(n,d)=(n+d-1)//d`を十分広い整数で計算する。

```text
serialize(f):
  data = hex_to_bytes(f.data_hex)
  if f.format == standard:
    m = [0] ++ bits(f.id,11) ++ [0,0,0] ++ bits(len(data),4)
  else:
    m = [0] ++ bits(f.id >> 18,11) ++ [1,1] ++ bits(f.id & 0x3ffff,18)
        ++ [0,0,0] ++ bits(len(data),4)
  for byte in data: m = m ++ bits(byte,8)
  c = 0
  for b in m:
    feedback = b XOR ((c >> 14) & 1)
    c = (c << 1) & 0x7fff
    if feedback == 1: c = c XOR 0x4599
  raw = m ++ bits(c,15)
  out = []; positions = []; last = none; run = 0
  for b in raw:
    out.append(b)
    run = run + 1 if b == last else 1; last = b
    if run == 5:
      positions.append(len(out)); out.append(1-b)
      last = 1-b; run = 1
  frame = out ++ [1,0,1,1,1,1,1,1,1,1]
  return WireFrame(m,c,out,positions,frame,len(frame),8*len(data))
```

## 2. 状態の所有権とイベント遷移

<a id="state-machine"></a>

```trace
{
  "id": "design-can#state-machine",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0001",
    "DIR-REQ-0033",
    "DIR-REQ-0035",
    "DIR-REQ-0038",
    "DIR-REQ-0039",
    "DIR-REQ-0040",
    "DIR-REQ-0041",
    "DIR-REQ-0045",
    "DIR-REQ-0046",
    "DIR-REQ-0058",
    "DIR-REQ-0097",
    "DIR-REQ-0101",
    "DIR-REQ-0103",
    "DIR-REQ-0104",
    "DIR-REQ-0105",
    "DIR-REQ-0106",
    "DIR-REQ-0107",
    "DIR-REQ-0108",
    "DIR-REQ-0111",
    "DIR-REQ-0112",
    "DIR-REQ-0113",
    "DIR-REQ-0115",
    "DIR-REQ-0120"
  ],
  "upstream": [
    "architecture#arch-domain-models"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 要素 | 所有データ・不変条件 |
| --- | --- |
| BusContext | バスごとに1個。Controller別のprocessing/queue/transmitting/receiver状態、Request表、Bus状態、候補dirty印、timer対応表を保持する。Controller/Busはcontextに入力を渡すadapter。単一event loopから順番に呼ぶ。注：nested callbackで相手モデルを変更する処理は対象外 |
| キュー | source別のpending request_id集合。優先比較は[arbitration](../specs/models/CANモデル詳細機能仕様書.md#arbitration)。SOFで1件だけ除去、敗者は保持。queue長とpending数を常に一致させる |
| 所有者 | transmitting/intermissionのBusはactive_requestを1件持つ。transmitting中のRequestはin_flight、intermission中はsuccess。Controller.transmittingはEOFでnullになり、Bus.active_requestはreleaseでnullになる |
| 原子的反映 | 各入力で変更するfieldとqueue項目だけのTransition delta、scheduler/計測効果batchを組み立て、モデル内の失敗可能な算術・容量・整合検査を先に完了する。その後deltaをモデルへ適用しcallbackを返す。核が効果batchを検証・commitして観測を公開する。核のcommit失敗時は実行全体を終了し、直前commitのjournalから結果を復元する。注：全queue複製、失敗後のモデル再開、任意モデルrollbackは対象外 |
| SOFの効果 | 勝者除去、Request→in_flight、attempts=1、bus→transmitting、予定EOF/release算出、EOFとreleaseの順でphase0予約、計測を一batchにする。算術・予約検証失敗なら全効果を未commitにする |
| 同報の効果 | EOFでRequest→success、bus→intermission、送信Controller.transmitting=nullを反映する。同じbatch内でsource以外を正規パス順に列挙しReceiver pending作成とobserved通知の予約を行う。成功と全受信通知予約は同じcommit単位を持つ |
| 停止 | [終了境界](../specs/実行詳細機能仕様書.md#termination)で未処理eventと状態をsnapshotに残す。finishでは確定snapshotを保持してcache/token等の実メモリを解放する。注：ユーザー入力によるCAN転送取消し・通信abortは現profile対象外 |

| 通知・phase | 条件 | commitされる状態と予約効果 |
| --- | --- | --- |
| Generate、1 | dispatcherで生成対象が確定 | Request processingを作成し全WireFrame計算値を添付、正delayならTxProcessedをready時刻phase0へ、0ならCanTxRequestを同phase1へ予約 |
| TxProcessed、0 | processingの該当要求 | 状態processingを保持したままCanTxRequestを現在時刻phase1へ予約。これがready容量判定を一回だけ起こす |
| CanTxRequest、1 | source/bus/frame/生成時刻が登録Requestと一致 | ready_psを記録。容量規則に従いpending格納又はdropped終端。格納成功時にbusをdirty指定する。idle中もphase1の全要求を先に判定する |
| Arbitrate、2 | dirtyかつidle | 各sourceの最良1件を集めて勝者を求めSOF batchをcommit。候補空なら状態維持。占有中なら状態維持しreleaseで再評価。敗者の格納順を維持する |
| Eof、0 | busがtransmittingかつrequest/token一致 | SOFの予定時刻でEOF batchをcommit。全ControllerはACK可能という準備前提を適用する |
| Release、0 | busがintermissionかつrequest/token一致 | release_ps記録、bus idle、active_request=null、busをdirty指定。後続phase1の新着を含め次のphase2で再仲裁 |
| CanNotification、1 | EOF commitで作成したReceiverがpendingかつ未observed | observed_ps記録、フィルタ不適合はfiltered。適合はpendingのまま正delayのRxProcessedをphase0へ予約、0delayはReceivedを同phase1へ予約 |
| RxProcessed、0 | Receiverが適合・処理中 | Receivedを現在時刻phase1へ予約。pending状態維持 |
| Received、1 | 同じReceiverでobserved済み | received_psを記録しreceived終端。Request successとバス状態は保持 |

| 境界 | 処理 |
| --- | --- |
| 同時刻満杯 | 新着CanTxRequestのphase1時点のqueueで判定する。注：同時刻phase2で空く予定を容量に加算する処理は対象外 |
| 解放時刻の後着 | release phase0、arrival phase1、arb phase2なので後着も次の候補へ入る。EOF直後の到着はintermission終了まで保持 |
| 遅い受信 | 観測や処理が次フレームのSOFを越えてもreceiverごとに独立して進める。source当人へのReceiverは作らない |
| 重複・不整合 | 二重EOF/ready/受信完了、state不適合、token不一致、unknown request、別source同一仲裁値はモデル不変条件違反としてexecution_failed。注：stale timerの再実行は対象外 |
| 時間切断 | EOFちょうどTはin_flight・Receiver0件。EOF<Tかつrelease>=Tはsuccess/intermission。observed=T又はreceived=Tはその処理を行う前のReceiver pendingを保持 |

| 初期状態のcanonical JSON | 正確な形 |
| --- | --- |
| Controller | `{"profile":"can.cc.ideal.v1","queue":[],"processing":[],"transmitting":null,"receivers":[],"generators":[{"id":"a","next_ordinal":"0","next_time_ps":"0"}]}`。generatorsはこのsourceに属する全generatorをID順に格納。次候補がcount/end/timesの終端ならnext_time_ps=null、T境界外の候補も存在すればu128の整数文字列を記録する（未予約の論理候補でありschedulerのu64時刻とは別）。generatorなしは空配列 |
| Bus | `{"profile":"can.cc.ideal.v1","state":"idle","active_request":null}` |
| channel | `{"delay_ps":"0"}`。確定delayを10進文字列で記録する |
| compound/network | `{}`。独立した可変モデル状態がない構造インスタンスを表す |
| 直列化 | 各objectのキーをUTF-8辞書順に整列したcompact UTF-8 JSON、ASCII識別子と10進文字列、空白・改行なしでstate文字列を作成する。上表はfield形状を示し、表示順は保存時のキー順を表さない。外側metadataは[結果仕様](../specs/結果詳細機能仕様書.md#experiment-metadata)に従う |

## 3. 型付きpayloadと内部API

<a id="payloads"></a>

```trace
{
  "id": "design-can#payloads",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0033",
    "DIR-REQ-0097",
    "DIR-REQ-0099",
    "DIR-REQ-0103",
    "DIR-REQ-0111",
    "DIR-REQ-0112",
    "DIR-REQ-0114"
  ],
  "upstream": [
    "architecture#arch-domain-models"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 型・境界 | 契約 |
| --- | --- |
| 外部payload | [CAN仕様controller節](../specs/models/CANモデル詳細機能仕様書.md#controller)のschema・codecを実装する。正確なwire fieldの定義は同節を正本とする |
| 内部Event | `Generate{generator_id,ordinal}`、`TxProcessed{request_id}`、`Eof{request_id}`、`Release{request_id}`、`RxProcessed{request_id,receiver_id}`、`Received{request_id,receiver_id}`。Eventにはscheduler所有tokenを対応付ける。これらはポートpayloadではなくCANモデル内のtimer種別 |
| request API | `on_generate(g,k)`、`on_tx_processed(id)`、`on_tx_request(payload)`、`on_arbitrate(bus)`、`on_eof(id)`、`on_release(id)`、`on_notification(payload)`、`on_rx_processed(id,receiver)`、`on_received(id,receiver)`は状態節の局所Transition/effects又はModelErrorを返す。モデル固有queueはBusContextが一元所有する |
| エラー分類 | codecの形式・field型検証失敗はE-0002/invalid_event、復号後にhandlerが検出した登録Request・宛先・時刻・状態との不一致はE-0002/model_failedとして診断する |
| 型登録の分担 | codecとhandler登録はsim-can、Envelope bytes運搬・token配送は共通核、結果のCanFields組立はsim-can。型追加は新schema/versionの登録で行い、既存CANのcodecを変更する場合はversionを更新する |

```json
{"schema_version":1,"profile":"can.cc.ideal.v1","request_id":"a:0","source_id":"Main.a","bus_id":"Main.bus","frame":{"format":"standard","id":0,"data_hex":""},"generated_ps":"0","ready_ps":"0"}
```

## 4. 準備検証と生成dispatcher

<a id="workload-validation"></a>

```trace
{
  "id": "design-can#workload-validation",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0001",
    "DIR-REQ-0006",
    "DIR-REQ-0007",
    "DIR-REQ-0024",
    "DIR-REQ-0045",
    "DIR-REQ-0058",
    "DIR-REQ-0097",
    "DIR-REQ-0098",
    "DIR-REQ-0099",
    "DIR-REQ-0100",
    "DIR-REQ-0101",
    "DIR-REQ-0102",
    "DIR-REQ-0103",
    "DIR-REQ-0114",
    "DIR-REQ-0116"
  ],
  "upstream": [
    "architecture#arch-domain-models"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 処理 | アルゴリズム・エラー境界 |
| --- | --- |
| 準備順 | 入力取得・NED/INI確定→profile/接続/bitrate/Controller数検査→JSON構造と全generator/frame検証→全generatorの送信者所有Map検証→frame serialize/cache→初期状態と次発火候補計算。違反は初期イベント実行前のprepare診断 |
| JSON型 | 整数は負号なし十進整数tokenを受理し、boolean・指数・小数tokenは型違反として診断する。dataは空hex可、JSON全階層で重複キーを検出する。kindごとの許可field集合で未知キーを診断する |
| 所有者Map | key=(bus,format,id)、value=source。empty times、count=0、T以後だけのgeneratorも走査する。既存valueと同sourceなら受理、別sourceは準備失敗 |
| dispatcher | generator cursorをID順に保持する。各cursorの次発火時刻を求め全体最小時刻tを選び、時刻tの全発火を収集し(generator ID,ordinal)順でGenerateをphase1へ予約する。処理済cursorを進め再び最小候補を求める。注：列を全期間展開する方式は対象外 |
| 候補算術 | [workload](../specs/models/CANモデル詳細機能仕様書.md#workload)の式をu128で評価しcount/endを先に適用する。有効候補がu64時刻を越え、T以内への予約が必要なら時間算術失敗。T以後は非実行の残候補として保持し、終了理由判定へ論理的に存在を伝える |
| ordinal | 生成済み最大値を次に進める演算はchecked。count=u64maxなら最大実行ordinal=u64max−1。回数無制限で次ordinalが表現不能となる前に停止境界を判定し、予約が必要な場合に算術失敗とする |
| 検証の分担 | [CAN検証仕様](../verification/cases/CANモデル検証仕様書.md)の0001～0006でbit列、遷移、生成、入力診断、所有権と失敗時のprefixを確認する。共通基盤・性能は[利用フロー・品質検証](../verification/cases/利用フロー・品質検証仕様書.md)で同じ入力から結果までを確認する |

### 設定・初期化・実行への負荷の接続

DIR-REQ-0006・0007・0024の分担として、dispatcherは取得済みworkload bytesとINIのT・処理遅延・ビットレートをPreparedSimulationから受け取る。factoryへ渡す確定値とserializerが使うbitrateを一致させ、初期化全件成功後にのみGenerateが発火する。未来の未生成要求を台帳へ加えず、入力検証ではT以後のgeneratorも含める。同じNEDに二つのINIを適用する比較はDIR-TEST-0004とDIR-TEST-0080で確認する。

### 中規模の負荷生成と性能の分担

DIR-REQ-0001・0058・0116の評価例を[DIR-TEST-0084](../verification/cases/利用フロー・品質検証仕様書.md#dir-test-0084)へ割り当てる。32送信元・1000000要求でもdispatcherは送信元ごとのcursorと次候補を保持し、全期間のイベントを先に予約せず、frame内容・bitrateで共有できるserializer結果をcacheする。待機中requestだけをモデルキューに保持し、確定した観測は結果処理のCSV writerへ順次渡す。集計の件数・和・最大・積分状態と実行のFES/キュー、出力済み行の保持を分離し、過去のCSV行を実行のために全件メモリへ保持する設計を避ける。停止時の未完了に必要な状態は残す。

現行実装の`run`は`runtime/can_archive.rs`の安定ハンドルと所有された台帳ファイルを使う。予約イベントの参照は成功callbackの確定後に解放し、GatewayのRX保持が終わるまで親要求を残す。EOFだけで退避せず、release・遅延受信・routing・子要求のTX入場まで待つ。停止時は残る状態を凍結する。直接の`runtime::simulate`は従来のメモリ上Snapshotを返す。`output/stream.rs`は台帳の寄与を時刻順へ整列し、現在窓と対象別の集計状態だけを更新する。

出力を含めたwallとRSSは外部計測、イベント処理だけのwallは内部計測とし、いずれも仮想時刻や優先度の計算へ入力しない。品質方針の120秒・2GiBは検証で測定する目標であり、上記データ構造だけで達成済みとは扱わない。モデル間の性能比較は要求数とevent数・抽象度を併記する。
