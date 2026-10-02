# AXIモデル検証仕様書

文書バージョン：`0.1.1`
対象GitHubバージョン：`v0.1`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：完全入力と独立解析値を用いるAXI検証6ケースを追加 |

文書ID：`verification-axi`

| 項目 | 内容 |
| --- | --- |
| 文書状態 | 契約確定。製品実装・製品実行試験は未実施 |

| 項目 | 確定契約 |
| --- | --- |
| 対象 | [AXI仕様](../../specs/models/AXIモデル詳細機能仕様書.md)、[AXI設計](../../design/AXIモデル詳細設計書.md)。単一clock、Manager2、Interconnect1、RAM1のprofile範囲を確認する |
| 入力 | [Main.ned](../fixtures/axi/models/demo/Main.ned)と[scenarios.json](../fixtures/axi/scenarios.json)が指す8組のINI/model-config/workload。全ファイルをfixture配下へ保存し、パス変更なしで指定できる |
| 実行 | rootから`dir-simulator run --config docs/verification/fixtures/axi/read-write.ini --output /tmp/dir-axi-read-write`。各caseのINIへ置き換え、一回ごとに存在しない出力名を選ぶ。製品CLI実行は未実施 |
| 照合 | results.json schema2 model_recordsからaxi.transaction/axi.handshake/axi.memoryを抽出しexpected projectionと比較。外側request_id/time_psとdataを合成して照合する。数値Dは整数へ変換する。status/response/byte/順序/psは完全一致。expected.metricsの平均・率は独立の有理数{numerator,denominator}で保存し、共通丸め規則でbinary64へ一度変換して比較する |
| 解析資料の確認 | `python3 docs/verification/fixtures/axi/verify_fixtures.py`はREADYの剰余計算、チャネル段階の算術、整数maskによるRAM変化を固定期待値と照合する。注：製品イベントループやRTLの実行結果の生成は対象外 |

<a id="dir-test-0017"></a>

## 1. WSTRB書込と後続read

```trace
{
  "id": "DIR-TEST-0017",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0137",
    "DIR-REQ-0138"
  ],
  "upstream": [
    "design-axi#memory"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 担当 | [memory](../../design/AXIモデル詳細設計書.md#memory)のbyte配列・read snapshot。DIR-FUNC-0031/0033、[DIR-AC-0033](../../要件定義書.md#dir-ac-0033) |
| 入力 | read-write.ini。RAM初期先頭8byte=0001020304050607、base4096。m0の2beat write=11223344/55667788、WSTRB=5/10、m1の2beat readを同時刻0に生成 |
| 期待データ | W0でbyte0/2を更新し11013303、W1でbyte1/3を更新し04660688、後続read_data=[11013303,04660688]。memory先頭8byte=1101330304660688、残り24byteは0。memory time_ps=30000 |
| 期待時刻 | P10ns、m0 grant0、AW10ns、W20/30ns、B40ns。m1 grant40ns、AR50ns、R60/70ns。両transaction completed/OKAY。WLAST/RLASTはbeat1だけtrue |
| zero strobe | zero-strobe.iniはW20nsを一件handshakeしB30ns completed、RAM初期値を保持、memory time_ps=0 |
| 判定 | 全ModelRecordを解析値と照合し、data_hexをlittle-endian wordとして読む値とbyte順を確認する。WSTRB無効byteの保持、空write副作用でも応答が一件あることを確認 |
| 実行状態 | 解析fixture照合済み。製品read/write実行は未実施 |

<a id="dir-test-0018"></a>

## 2. round-robinとManager容量

```trace
{
  "id": "DIR-TEST-0018",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0139",
    "DIR-REQ-0140"
  ],
  "upstream": [
    "design-axi#channels",
    "design-axi#prepare"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 担当 | [channels](../../design/AXIモデル詳細設計書.md#channels)のcursorと[prepare](../../design/AXIモデル詳細設計書.md#prepare)の生成容量。DIR-FUNC-0031/0032、[DIR-AC-0031](../../要件定義書.md#dir-ac-0031)、[DIR-AC-0032](../../要件定義書.md#dir-ac-0032) |
| 入力 | round-robin.ini。m0 a:0/a:1、m1 b:0の1beat readを時刻0、max_outstanding=2。capacity.iniはm0 max_outstanding=1で2件を時刻0に生成 |
| 期待 | round-robin grant順a:0,b:0,a:1、grant0/20/40ns、完了20/40/60ns。cursorはm0選出後m1、m1選出後m0。capacityはa:0 completed、a:1 dropped/outstanding_full、a:1 handshake0件 |
| 境界追加 | capacityのa.timesを[0ps,20ns]へ変更するとresponse完了phase0で容量が空き、同時刻phase1のa:1が受理される。a:1 grant20ns、AR30ns、R40ns。[0ps,19999ps]なら二件目は破棄 |
| 不変条件 | 各commitでsource別outstanding=pending+active、FIFO=そのsourceのpendingだけ。global active<=1。同managerのa:0完了前にa:1を開始する処理は対象外（注） |
| 実行状態 | 固定fixtureの順序・容量期待値を静的照合済み。製品調停と境界追加実行は未実施 |

<a id="dir-test-0019"></a>

## 3. 五チャネルとREADY待ち

```trace
{
  "id": "DIR-TEST-0019",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0136",
    "DIR-REQ-0141"
  ],
  "upstream": [
    "design-axi#channels"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 担当 | [channels](../../design/AXIモデル詳細設計書.md#channels)のnext-ready剰余計算とpayload保持。DIR-FUNC-0032、[DIR-AC-0032](../../要件定義書.md#dir-ac-0032) |
| 入力 | backpressure.ini。aw_ready=10、w_ready=001、m0.b_ready=01、m1.r_ready=10、read/write response latency=2cycle、他ready=1 |
| 期待handshake | AW20ns、W50/80ns、B110ns、AR120ns、R140/160ns。valid_sinceは順に10/10/60/100/120/140/150ns。RAM/read値はread-writeと同じ。READY待ちcycleは1/4/2/1/0/0/1 |
| 保持確認 | AWとW0のVALIDがgrant+1から独立に有効であること、WREADYがAW handshakeの次edgeまで0であることを確認。各stageで最早edge直前のpayloadを捕捉しREADY0区間とhandshake時の値が一致することをunit testで確認。R/Wのbeat番号とLASTを照合し、READY0で計測byte数が増加する処理は対象外（注） |
| 非整列生成 | read-writeの全timesを1psへ変更するとeligible10ns、最初のgrant10ns、AW20nsになる。generated_psは1のまま保持する |
| 判定 | 期待edgeから求めたpsと出力を完全一致させる。イベントを省略したREADY0区間でもvalid_sinceとhandshake差を保持する |
| 実行状態 | 剰余式と段階依存から解析値一致。製品clock/codec/保持試験は未実施 |

<a id="dir-test-0020"></a>

## 4. DECERR・SLVERRと副作用

```trace
{
  "id": "DIR-TEST-0020",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0142"
  ],
  "upstream": [
    "design-axi#memory"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 担当 | [memory](../../design/AXIモデル詳細設計書.md#memory)の範囲decodeとエラー完了。DIR-FUNC-0033、[DIR-AC-0033](../../要件定義書.md#dir-ac-0033) |
| 入力 | responses.ini。error範囲[4100,4104) access=both、m0 A4096の2beat write、m1同read、m0 A8192の1beat read |
| 期待 | a:0はSLVERRでB40nsまで全Wを消費、RAM初期値を保持。b:0はSLVERR、R2件とも00000000、70ns完了。c:0はDECERR、R1件00000000、90ns完了。termination events_exhausted、exit0 |
| 追加境界 | writeストローブ全0でもA4096/2beatはSLVERR。error範囲access=readなら同writeはOKAY、readはSLVERR。RAM末尾を越えるvalid burstはDECERRを優先、4KiBを越えるburstはprepare失敗として別扱い |
| 判定 | response code、R beat数/LAST、B一件、全error writeのmemory差分0byteを比較。protocol errorと実行失敗を別の出力として保持する |
| 実行状態 | 区間交差と応答・副作用の解析値確認済み。製品error処理は未実施 |

<a id="dir-test-0021"></a>

## 5. 完全入力と準備境界

```trace
{
  "id": "DIR-TEST-0021",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0133",
    "DIR-REQ-0134",
    "DIR-REQ-0135",
    "DIR-REQ-0144"
  ],
  "upstream": [
    "design-axi#prepare"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 担当 | [prepare](../../design/AXIモデル詳細設計書.md#prepare)のprofile、ports、型検証。DIR-FUNC-0031/0032/0033、[DIR-AC-0031](../../要件定義書.md#dir-ac-0031)、[DIR-AC-0033](../../要件定義書.md#dir-ac-0033) |
| 正常入力 | 8組すべてprepareし、Manager2/Interconnect1/Ram1、15本の対応する接続、clock10000ps、RAM32byteを得る。256beat@0と1beat@0xfffffffcはframe入力として受理（RAM範囲外の応答は実行時DECERR） |
| 異常入力 | [invalid-transactions.json](../fixtures/axi/invalid-transactions.json)の9個のmutationをread-writeのgenerator a.transactionへ一つずつ適用。unaligned、4KiB跨ぎ、beats0/257、strobe16、data個数不足、奇数hex、burst上書き、boolean addressを拒否 |
| 設定・構造異常 | モデルJSONのREADYを000、clockを0ps、latency0、RAM initial重複、error_range重複へ個別変更。NEDのm1.rをm0.rへ接続変更、W方向反転、channel delay1ps、suffixの混在、未接続を個別注入してprepare失敗を確認 |
| 判定 | prep_failed/exit2/E-0001、該当sourceとfield又はpath、callback0件。受理データと将来拡張の未対応入力を区別する。profile/実装版/全入力snapshotが結果から参照できることを確認 |
| 実行状態 | 9mutationの解析validatorと2受理境界は確認済み。DIR parser/registry/prepare実装は未実施 |

<a id="dir-test-0022"></a>

## 6. 打切り・journal・統合再現性

```trace
{
  "id": "DIR-TEST-0022",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0133",
    "DIR-REQ-0143",
    "DIR-REQ-0144"
  ],
  "upstream": [
    "design-axi#records"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 担当 | [records](../../design/AXIモデル詳細設計書.md#records)の確定prefixと資源解放。DIR-FUNC-0031/0032/0033、[DIR-AC-0032](../../要件定義書.md#dir-ac-0032)、[DIR-AC-0033](../../要件定義書.md#dir-ac-0033) |
| 入力 | stop-mid-write.ini(T30ns)とstop-response.ini(T40ns)、加えて全8fixtureを同条件で二回実行する |
| 期待 | T30nsではAW10ns/W0 20nsだけ到達し、memory先頭1101330304050607、memory時刻20ns。T40nsではW1まで到達し先頭1101330304660688、時刻30ns。両方a:0 active/response OKAY/completed null、b:0 pending、time_limit |
| 指標descriptor | [metrics.json](../fixtures/axi/metrics.json)の24個をmetadata.metricsと完全一致させる。metric_id/version/unit/value_kind/sampling/aggregationの6キーだけを持ち、純AXIの出力に未登録IDがないことを確認する |
| 指標期待値 | scenarios.jsonのexpected.metricsは8scenario計424件のsummary projectionと11窓を保持する。read-writeはW25ns、busy率は窓順1,1,4/5,0、全体7/10。written_bitsは窓順16,16,0,0、read_bitsは0,0,64,0。m0/m1 queue_mean=0,2/5、outstanding_mean=2/5,7/10、両queue_max/outstanding_max=1 |
| 指標の点と母集団 | read-writeのwait点はm0=0ps/m1=40000ps、latency点は40000/70000ps、$all平均はwait20000ps/sample_count2、latency55000ps/sample_count2。W0のchannel_stall_cycles=1、reason=W、request_id=a:0。capacityの同時生成ではdropでもgauge同値の点が一組出る。responsesのerror全beatのread_bits/written_bitsは0 |
| ゼロと打切り指標 | T0psへ変更した実行はsummary件数/量/最大0、平均/使用率/率null、標本数0、point/窓0件。空負荷かつH>0では使用率/時間平均/量/率0、標本平均null。stop-mid-writeのsummaryはcompleted0/active1/pending1、written_bits16、busy1、latency平均null。異常prefixはHより後の占有を加算する処理は対象外（注） |
| 件数 | 各scenarioでgenerated=completed+dropped+pending+active。B又はRLASTがTちょうどならactiveを保持。handshake.time_ps<Tを全件確認。partial=falseの通常打切りとexecution_failedのpartial=trueを区別 |
| 失敗注入 | W0 callbackの次event予約検証を失敗させ、直前AWまでのjournalを出力する。handshake W0行0件、RAM初期値、transaction activeの確定prefix、primary errorとpending eventを確認。finishは一回で内部資源を解放しsnapshotを保持する |
| 再現性・拡張 | 各8fixtureの2回結果からrun_id/実時刻情報を除いたmodel_recordsを完全一致させる。CAN fixtureのschema1を回帰実行し既存値・順序が維持されることを確認。別AxiContextのmemory更新が他contextへ伝播しないことをunit testで確認 |
| 実行状態 | T境界での解析handshakeとメモリ差分は照合済み。製品停止・失敗注入・CAN回帰・拡張実行は未実施 |

## 7. 検証記録

| 項目 | 確定契約 |
| --- | --- |
| 2026-09-29解析 | verify_fixtures.pyで8scenario、9異常mutation、2受理境界、24metric descriptor、424summary projection、11窓を照合しPASS。標準ライブラリだけを使用する。固定edge列は実装から生成した値ではなくプロジェクトの解析fixture |
| 製品証跡 | 製品実装・RTL・外部AXI conformance testは未実行。実装後にcommit/環境/入力hash/期待値/実測値/合否/出力所在を追加する |
| 資料 | 一次資料と確認した版・範囲は[AXI仕様の根拠](../../specs/models/AXIモデル詳細機能仕様書.md)を正本とする |
