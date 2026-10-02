# AXIモデル詳細設計書

文書バージョン：`0.1.1`
対象GitHubバージョン：`v0.1`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：AXI状態・仲裁・転送・RAM・結果の内部責務を確定 |

文書ID：`design-axi`

| 項目 | 内容 |
| --- | --- |
| 文書状態 | 契約確定。製品実装・製品実行試験は未実施 |

| 項目 | 確定契約 |
| --- | --- |
| 担当元 | [architecture#arch-axi](../アーキテクチャ設計書.md#arch-axi)のsim-axi責務。外部規則は[AXI仕様](../specs/models/AXIモデル詳細機能仕様書.md)を正本とする |
| 共通基盤 | [実行設計](実行詳細設計書.md)のeffect batch、phase、timer、journalを使用する。CAN側のpayload/状態は変更対象外（注） |

## 1. 構成・所有権とprepare

<a id="prepare"></a>

```trace
{
  "id": "design-axi#prepare",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0133",
    "DIR-REQ-0134",
    "DIR-REQ-0135",
    "DIR-REQ-0140",
    "DIR-REQ-0144"
  ],
  "upstream": [
    "architecture#arch-axi"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| AxiContext | Interconnect単位にclock、path順Manager/FIFO/outstanding、round-robin cursor、active transaction、channel段階、Ram byte列とerror判定、要求表を所有する。adapterは同一contextに通知を渡す。別contextの状態は別所有とする |
| prepare | 共通NED/INI解決→profile configキー/型→3実装型と5経路の対応→clock/RAM/pattern/容量→全generator/transaction→initial RAM構築→初期journalの順に検証する。全検査成功前のcallback実行は対象外（注） |
| 生成 | generatorを(time,id,ordinal)順に遅延展開する。t<TをGenerate phase1へ予約。生成時の上限判定結果とeligible時刻をTransactionレコードへ格納する。pending受理時だけFIFOへ追加する |
| 初期状態 | 各Manager FIFO空/outstanding0、Interconnect cursor0/active=null、RAMは零埋め後initialを反映。metadata.initial_stateはcanonical JSONでManager={queue:[],outstanding:"0"}、Interconnect={cursor:"0",active:null}、Ram={base:D,size:D,data_hex:S}、構造wrapper={}。キーは辞書順 |
| 内部event schema | `axi4.transaction.v1.Generate`版1 phase1 payload={generator_id:S,ordinal:D}、`Wake`版1 phase0 payload={interconnect:S}、`Handshake`版1 phase0 payload={request_id:S,channel:S,beat:D?}。全payloadはcanonical JSON、予約tokenをcontextのstageへ対応付ける |
| ポートと実行 | 5経路は型・source/targetの対応と論理handshake payloadを表す。共有AxiContextがHandshake timerで両端の状態を一括更新する。注：同じtransferに別send_atを重ねて二回配送・遅延加算する処理は対象外。核はCAN同様に不透明なtimer/効果だけを扱う |
| 失敗 | prepareの不正はE-0001、event codec不正はE-0002 invalid_event、復号後state不一致はmodel_failed、時間/連番あふれはE-0004。型・時刻を持つ未commit効果は共通の失敗prefix規則に従う |

## 2. edge計画とround-robin

<a id="channels"></a>

```trace
{
  "id": "design-axi#channels",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0136",
    "DIR-REQ-0139",
    "DIR-REQ-0141"
  ],
  "upstream": [
    "architecture#arch-axi"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 待機予約 | pendingのeligibleが現在edgeならrequest_arbitration、futureなら最小eligibleへWake timer phase0を一件だけ予約する。既存Wakeより早いeligibleが到着すれば旧tokenを取消し最小edgeへ置換する。Wakeは自身のtokenを消費してdirtyを指定する。grant時に残る不要Wakeを取消す。注：active中は完了edgeで再選出するため重複Wake生成は対象外 |
| 選出 | phase2でidleを確認しcursorからN件まで巡回、eligible先頭を持つ最初をpopする。outstandingは維持。active設定とcursor更新、最初のchannel予約とtransaction更新を一effect batchにする |
| handshake計画 | W0は最早VALIDをgrant+1で保存し、READY探索の開始edgeをAW handshake+1にする。他stageは最早VALIDから探索する。探索開始edge e、pattern長Lに対しj=0..L−1で最初にpattern[(e+j)%L]=1を選びh=e+j。valid_sinceは最早VALID*P、実予約=h*P。patternには1があるので探索はL以下。予約はphase0。注：idleの各clockやREADY=0のedgeごとのイベント生成は対象外 |
| 状態遷移 | write: Idle→AwWait（W0 VALID/payloadも同時に保持）→WWait(0..beats−1)→BWait→Idle。read: Idle→ArWait→RWait(0..beats−1)→Idle。各stageの最早edge/latencyは仕様channels節から算出する |
| phase0 callback | request/stage/beat/tokenを照合→局所Transitionとレコードを構築→次stage時刻と予約検証→対象state更新とeffect蓄積→共通核が一括commit。B/RLASTではoutstanding−1、active=null、dirty指定する |
| 保持 | 予約されたhandshakeまでpayloadとvalid_sinceをimmutableに保持する。生成と新着高優先度はactive転送を変更する対象外（注）。RamからのRデータは当該stage開始時snapshotで固定する |
| 上限・停止 | n*P及び加算をu128で評価してu64へ値域検査する。AXIのeligible/handshake予定はT以後も表現可能な値をFESへ予約して未処理予定として保持し、u64超過はE-0004で実行失敗とする。t=Tは処理対象外（注）。同時刻のphase0完了→phase1生成→phase2再選出の順を固定する |

## 3. byte演算と応答

<a id="memory"></a>

```trace
{
  "id": "design-axi#memory",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0137",
    "DIR-REQ-0138",
    "DIR-REQ-0142"
  ],
  "upstream": [
    "architecture#arch-axi"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| byte配列 | Ramはsize長のu8配列、index=address−base。prepare時に初期化。外側の4byte word整数へ依存せず、hexから得たbyte[j]とstrbのbitjで更新する |
| decode | AW/ARの直後にexclusive end=A+4*beatsを広い整数で算出。A<base又はend>base+sizeならDECERR。そうでなければaccess適合rangeとの半開区間交差を検査しSLVERR又はOKAY |
| 書込delta | OKAY W beatについて最大4個の(index,new byte)deltaを作り、ハンドシェイク行/メモリsnapshot/次eventを同じbatchへ記録。errorではdelta空。strb0でもhandshakeとbeat前進を記録する |
| 読出しdelta | OKAYは4byteコピー、errorは4個zeroからR payloadを構成する。handshake成功時にread_dataへその8桁hexをappend。最終R以外の完了status更新は対象外（注） |
| 不変条件 | activeは最大1、生成済み要求は相互排他status一つ、outstanding=source別pending+active、FIFOはpendingだけ、accepted W/R beat番号は0始まり連続、lastは最終だけ。error writeのRAM差分は空 |

## 4. 結果・資源解放と拡張

<a id="records"></a>

```trace
{
  "id": "design-axi#records",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0133",
    "DIR-REQ-0143",
    "DIR-REQ-0144"
  ],
  "upstream": [
    "architecture#arch-axi"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 記録 | [AXI仕様records](../specs/models/AXIモデル詳細機能仕様書.md#records)の3schemaへ変換。AxiContextだけがmodel_records差分を発行し、核は型識別と共通wrapperだけを管理する |
| 指標登録・計測 | [AXI指標表](../specs/models/AXIモデル詳細機能仕様書.md#records)の24descriptorをprofile選択時に登録する。AxiContextがGenerate/grant/handshakeの同じTransitionでgauge・標本・bit増分・busy変更を順序付き効果へ含め、共通集計器が窓境界・正確積分・終端件数を出力する。窓ごとのtimerは生成対象外（注） |
| 集計の所有権 | 計測器はManager別Q/Oの前値と最終変化時刻・最大、Interconnectのbusy区間とbeat量、wait/latencyの和と件数を保持する。ModelRecordと計測の効果batchを共にcommitし、失敗時は両方を同じprefixへ切り戻した結果をjournalから得る |
| メモリ効率 | 内部RAMを毎event複製する処理は対象外（注）。journalは変更byte又はchunk差分を受け取り、export時だけ全data_hexを組み立てる。full snapshotの外部意味を維持する |
| 失敗prefix | 局所の失敗可能な値・整合検査後にstate適用する。共通effect commitが失敗したらglobal stopし、最後の確定journalからモデル結果を作成する。任意モデルrollbackと再開は対象外（注） |
| finish | snapshotを保持しtimer/context/cache/RAM実メモリを解放する。finishから追加転送やmemory書換えを観測へ発行する処理は対象外（注） |
| 検証割当 | [AXI検証仕様](../verification/cases/AXIモデル検証仕様書.md)のDIR-TEST-0017～0022でprepare、state、RAM、backpressure、停止を確認。将来profileは既存fixturesを回帰に使用する |
