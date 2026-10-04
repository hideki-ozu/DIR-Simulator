# Ethernet媒体拡張検証仕様書

文書バージョン：`1.1.2`
対象GitHubバージョン：`main @ 7bb9bfc`
予定公開版：`v1.1.2`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-04` | 20媒体fixtureの製品結果checker、内部・viewer回帰試験とソース／成果物hash付き実行記録を追加 |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。10/100半二重CSMA/CDと1000BASE-T1全二重媒体を定義 |

文書ID：`verification-ethernet-media`

| 項目 | 内容 |
| --- | --- |
| 状態 | 検証仕様確定。静的fixtureの独立整数解析と開発中ソースの製品試験を区別して記録する。実機・規格適合試験は未実施 |
| 入力 | [scenarios.json](../fixtures/ethernet-media/scenarios.json)に20組のNED/INI/model-config/workloadと解析期待projectionを列挙。全ケースで[Types.ned](../fixtures/ethernet-media/models/media/Types.ned)とMain又はMixを使用 |
| 静的実行 | `python3 docs/verification/fixtures/ethernet-media/verify_expectations.py`。JSON重複、file参照、引用profile、媒体対/速度/role/伝搬境界、時間式、SHA256固定15vector、16試行境界を照合 |
| 製品実行 | `dir-simulator run --config docs/verification/fixtures/ethernet-media/collision.ini --output /tmp/dir-media-collision`。各INIと未存在の出力ディレクトリに置換し、[verify_results.py](../fixtures/ethernet-media/verify_results.py)の`--result NAME=OUTPUT_ROOT`を全20ケース分指定する。schema2のmodel_records・metric・停止状態を独立期待値へ照合。[実行記録](#product-execution)に開発中ソースの結果を保存 |
| 判定 | 整数ps・件数・ID・状態は完全一致。率と平均は共通binary64規則。bit数/FCSは既存Ethernet独立vector、CSMA時間は手計算可能な64byte/576wirebitを使用し製品から期待値を生成しない |

<a id="dir-test-0033"></a>

## 1. carrier・伝搬衝突・jam

```trace
{
  "id": "DIR-TEST-0033",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0161",
    "DIR-REQ-0162",
    "DIR-REQ-0163",
    "DIR-REQ-0166"
  ],
  "upstream": [
    "design-ethernet-media#half-state"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [half-state](../../design/Ethernet媒体拡張詳細設計書.md#half-state) |
| 上流 | [DIR-FUNC-0039](../../機能仕様書.md#dir-func-0039)、[DIR-AC-0039](../../要件定義書.md#dir-ac-0039)、[csma](../../specs/models/Ethernet媒体拡張詳細機能仕様書.md#csma) |
| 入力 | collision、carrier、arrival-tie、late-start、zero-propagation、half-10の各INI |
| 期待 | 100Mbps/P100ns/両SOF0は検出100ns、preamble完了640ns、jam終了960ns、peer信号終端1060ns。carrierケースのB generated200nsとarrival-tieの100nsは既存A carrierを見てSOF6820nsへ待機し衝突0 |
| 境界 | B SOF50nsならA検出150ns/B検出100ns、jam終了A960ns/B1010ns。P0は両SOF0→次delta同時刻collision0、字句順の片側勝者を作らない。10Mbpsは同じbit/伝搬比率で時間10倍 |
| 内部確認 | collision後の予定EOF/正常Arrivalが成功量・reception・committed eventへ現れない。同時刻PairTimelineの他端IFG/EOFを一方の取消しで削除しない。登録順を反転して同じ論理結果を確認 |
| 結果 | 解析fixtureと製品結果を照合済み。実施した内部試験と残る検証範囲は[実行記録](#product-execution)を参照 |

<a id="dir-test-0034"></a>

## 2. BEB seedと試行上限

```trace
{
  "id": "DIR-TEST-0034",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0164",
    "DIR-REQ-0166"
  ],
  "upstream": [
    "design-ethernet-media#half-state"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [half-state](../../design/Ethernet媒体拡張詳細設計書.md#half-state) |
| 上流 | [DIR-FUNC-0039](../../機能仕様書.md#dir-func-0039)、[DIR-AC-0040](../../要件定義書.md#dir-ac-0040)、[csma](../../specs/models/Ethernet媒体拡張詳細機能仕様書.md#csma) |
| 入力 | collision.ini seed1、collision-repeat.ini seed0、[backoff-vectors.json](../fixtures/ethernet-media/backoff-vectors.json)、[attempt-limit-unit.json](../fixtures/ethernet-media/attempt-limit-unit.json) |
| 期待 | seed1の初回rはA0/B1、期限960/6080ns。A retry2020ns/EOF7780ns、Bは期限をcarrier busy中にも消費して8840nsにretry。seed0初回は両r1で6080ns再衝突、2回目rはA3/B0。digestとk=10上限を全15vectorで照合 |
| 16試行 | 内部MediaPair単体試験だけで乱数providerをr=0へ置換し、両MACを対向衝突させる。16回目SOF30300000ps、jam終了31260000psでattempt_limit。17回目予約・16回目後の抽選なし。注：このtest doubleを利用者model-configへ追加することは対象外 |
| 独立性 | 固定digest値、標準hashlibによる復算、slot/IFG/preambleの整数式を照合。製品schedulerやPRNGを期待値生成に使わない |
| 結果 | 解析fixtureと製品結果を照合済み。実施した内部試験と残る検証範囲は[実行記録](#product-execution)を参照 |

<a id="dir-test-0035"></a>

## 3. retry FIFO・容量・停止prefix

```trace
{
  "id": "DIR-TEST-0035",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0165",
    "DIR-REQ-0167"
  ],
  "upstream": [
    "design-ethernet-media#half-state",
    "design-ethernet-media#journal"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [half-state](../../design/Ethernet媒体拡張詳細設計書.md#half-state)、[journal](../../design/Ethernet媒体拡張詳細設計書.md#journal) |
| 上流 | [DIR-FUNC-0039](../../機能仕様書.md#dir-func-0039)、[DIR-AC-0040](../../要件定義書.md#dir-ac-0040)、[records](../../specs/models/Ethernet媒体拡張詳細機能仕様書.md#records) |
| 入力 | queue-retry.ini、stop-jam.ini（T800ns）、stop-backoff.ini（T1200ns） |
| 期待 | retry中a:0はcurrentとして容量外、200nsのa:1が容量1を占め300nsのa:2だけqueue_full。T800nsはjamming/jam_end=null、各outputのjam積分160000ps・送信積分800000ps、reception0。T1200nsはA deferred/B backoff、jam積分320000psずつ |
| 追加境界 | 生成/衝突/EOF/jam_end/backoff/Arrival各境界のT、T±1psを確認。capacity0は初回offerでdrop、attempt行0。currentをqueueへ戻して容量を消費する実装を拒否 |
| 失敗prefix | collision batch、pair timer rearm、fanout batchの予約失敗を注入し直前journalのみを保存。古い取消tokenがevent数/残件へ算入されず、retry generationが別の成功済attempt Arrivalを無効化しないことを確認 |
| 結果 | 解析fixtureと製品結果を照合済み。実施した内部試験と残る検証範囲は[実行記録](#product-execution)を参照 |

<a id="dir-test-0036"></a>

## 4. 混在Switch領域と入力診断

```trace
{
  "id": "DIR-TEST-0036",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0161",
    "DIR-REQ-0166",
    "DIR-REQ-0168"
  ],
  "upstream": [
    "design-ethernet-media#half-state",
    "design-ethernet-media#journal"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [half-state](../../design/Ethernet媒体拡張詳細設計書.md#half-state)、[journal](../../design/Ethernet媒体拡張詳細設計書.md#journal) |
| 上流 | [DIR-FUNC-0039](../../機能仕様書.md#dir-func-0039)、[DIR-AC-0039](../../要件定義書.md#dir-ac-0039)、[DIR-AC-0042](../../要件定義書.md#dir-ac-0042)、[half-config](../../specs/models/Ethernet媒体拡張詳細機能仕様書.md#half-config) |
| 入力 | mixed.ini、invalid-slot.ini、slot-upper.ini |
| 期待 | Mixのa half100Mbpsとb T1がSOF0。b→sw→cのfull出力SOF577000ps、a→sw→bのT1出力5860000ps。half出力sw→aはcarrier待ちで6820000ps。c次copyは7297000ps。同じSwitchでも対ごとの進行を維持 |
| 境界診断 | 100Mbps/P2400000psは2P+32bit=slotで準備失敗、P2399999psは受理。half1Gbps、速度/遅延片方向不一致、duplicate/missing physical pair、hub/multi-drop、未知PHY、half非zeroPHY latencyをprepareで拒否 |
| 判定 | 半二重対の衝突やretryが他対の所有状態を変更せず、失敗frameはSwitch転送へ進まない。v1 tree/flood/FDB制約を共用 |
| 結果 | 解析fixtureと製品結果を照合済み。実施した内部試験と残る検証範囲は[実行記録](#product-execution)を参照 |

<a id="dir-test-0037"></a>

## 5. T1同時双方向とrole

```trace
{
  "id": "DIR-TEST-0037",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0169",
    "DIR-REQ-0170",
    "DIR-REQ-0172"
  ],
  "upstream": [
    "design-ethernet-media#t1-state",
    "design-ethernet-media#journal"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [t1-state](../../design/Ethernet媒体拡張詳細設計書.md#t1-state)、[journal](../../design/Ethernet媒体拡張詳細設計書.md#journal) |
| 上流 | [DIR-FUNC-0040](../../機能仕様書.md#dir-func-0040)、[DIR-AC-0041](../../要件定義書.md#dir-ac-0041)、[t1](../../specs/models/Ethernet媒体拡張詳細機能仕様書.md#t1) |
| 入力 | t1-duplex.ini、invalid-t1-half.ini、invalid-t1-roles.ini |
| 期待 | 両方向sof0/eof576000/release672000ps、collision0。A→B到達877000ps、B→A1277000ps。master/slaveは構成属性でどちらも送信できる |
| 診断 | T1half、両master、両slave、role none、R100Mbpsを準備失敗。PHY行のrole/latency/rate/link_upとmetadata.sourcesを照合。training時間や750MBdをMAC時間へ加算しない |
| 結果 | 解析fixtureと製品結果を照合済み。実施した内部試験と残る検証範囲は[実行記録](#product-execution)を参照 |

<a id="dir-test-0038"></a>

## 6. T1 PHY pipeline・最大frame・停止

```trace
{
  "id": "DIR-TEST-0038",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0171",
    "DIR-REQ-0172"
  ],
  "upstream": [
    "design-ethernet-media#t1-state",
    "design-ethernet-media#journal"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [t1-state](../../design/Ethernet媒体拡張詳細設計書.md#t1-state)、[journal](../../design/Ethernet媒体拡張詳細設計書.md#journal) |
| 上流 | [DIR-FUNC-0040](../../機能仕様書.md#dir-func-0040)、[DIR-AC-0041](../../要件定義書.md#dir-ac-0041)、[t1](../../specs/models/Ethernet媒体拡張詳細機能仕様書.md#t1) |
| 入力 | t1-pipeline.ini、t1-max-frame.ini、t1-boundary.ini、t1-after-boundary.ini |
| 期待 | pipelineは同outputのSOF0/672000、release672000/1344000、到達2577000/3249000ps。次SOFより遅い旧attempt Arrivalを2件とも受理。最大MAC1518byteはEOF12208000/release12304000/arrival12509000ps |
| 停止 | T877000psはplanned_arrival877000/実arrival=null/reception0、T877001psはreceived1。planned_mdi/peer_mdi欄がT後でも予定として保存され実イベント扱いにならない |
| 処理遅延 | TX PHYとsource txProcessing、RX PHYとreceiver rxProcessingを独立に1ps増やし、各成分が一度だけ加算されMAC IFGへPHY成分が混入しないことを製品で確認 |
| 結果 | 解析fixtureと製品結果を照合済み。実施した内部試験と残る検証範囲は[実行記録](#product-execution)を参照 |

<a id="dir-test-0039"></a>

## 7. schema・metric・v1互換

```trace
{
  "id": "DIR-TEST-0039",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0167",
    "DIR-REQ-0168",
    "DIR-REQ-0172",
    "DIR-REQ-0173"
  ],
  "upstream": [
    "design-ethernet-media#journal"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [journal](../../design/Ethernet媒体拡張詳細設計書.md#journal) |
| 上流 | [DIR-FUNC-0041](../../機能仕様書.md#dir-func-0041)、[DIR-AC-0042](../../要件定義書.md#dir-ac-0042)、[records](../../specs/models/Ethernet媒体拡張詳細機能仕様書.md#records) |
| 入力 | 全20fixture、[完全行例](../fixtures/ethernet-media/record-examples.json)、既存Ethernet fixtures |
| 期待 | schema集合はframe1/reception1/transfer2/attempt1/phy_link1。transferの試行数・衝突数、offered保存則、成功arrivalとreceptionの一対一、PHY固定行、MDI予定field、9metric descriptorを照合 |
| 回帰 | `python3 docs/verification/fixtures/ethernet/verify_fixtures.py`を実行。v1/v2を同じbuildで選択しv1の時刻・FCS・3schema・metric集合を保持する試験を実施。範囲は実行記録を参照。v1へphysical_links等を暗黙追加する処理は対象外 |
| 追跡 | `python3 scripts/check_traceability.py --strict --requirement DIR-REQ-0161`を0162～0173にも適用し、親0145の全分担も確認。静的PASSを製品実行・規格適合の合格と呼ばない |
| 結果 | 解析fixtureと製品結果を照合済み。実施した内部試験と残る検証範囲は[実行記録](#product-execution)を参照 |

<a id="product-execution"></a>

## 8. 開発中ソースの製品実行記録

2026-10-04、媒体v2・専用100BASE-T1・CAN FDを追加した作業ツリーをビルドして実行した。対象ソース・入力・成果物のSHA-256、コマンドと実施範囲は[実行記録JSON](../results/ethernet-media-canfd-2026-10-04.json)に保存する。v1.1.2向けPRとして公開準備し、tagの作成は未実施。

| 実施範囲 | 結果と証跡 |
| --- | --- |
| 媒体全20ケース | validate/runで17件を受理し、invalid-slot・invalid-t1-half・invalid-t1-rolesの3件を期待どおりprepare拒否。[製品結果checker](../fixtures/ethernet-media/verify_results.py)で時間・状態・保存則・5schema・9metric descriptor・診断ruleを照合。準備失敗はCLI診断のみで、schema2結果を公開しない |
| 半二重内部試験 | [Rust結合試験](../../../crates/dir-simulator/tests/ethernet_media.rs)でcarrier、同時刻、jam/BEB、retry容量、停止prefix、取消予約の残件数、IFG期限でのcarrier再検出を確認。[媒体単体試験](../../../crates/dir-simulator/src/runtime/ethernet/media.rs)で15固定BEB vector、16衝突の上限と余分な抽選の不在、同時刻取消の独立性を確認 |
| T1と混在Switch | 双方向、旧attemptの遅延Arrival、最大frame、到達=T/T+1ps、Switch各対の独立性、100BASE-T1専用policyを照合。両方向SOF前のoverflowでいずれのSOFもcommitしない試験を実施 |
| 互換性とCLI | 同じbuildで既存CAN/GW/L2/QoS/VLANを含む全17 exampleをvalidate/run/view。新規6例を含む。全Rust試験275件成功（媒体結合14件を含む） |
| viewer | [モデル試験](../../../tests/media_fd_viewer_model.test.cjs)でjam・backoff/deferred・初回carrier待ち・FIFO・停止・巻き戻し・backoff期限のイベントステップを確認。[ブラウザ試験](../../../tests/media_fd_viewer_browser.cjs)と既存Ethernet/CAN/GWブラウザ試験が成功。全JavaScript試験108件中106件成功・2件skip |
| 文書と解析 | 独立解析20ケース・15BEB vector・16試行境界、strict traceability（205要件・54機能・245node）、生成資料と11図の整合を確認。これは製品試験とは別の証拠 |

公開Registry/Context/Envelope API全体、全callbackの予約失敗注入、全時刻のT±1ps・処理遅延の各成分を1ps変更する網羅試験、実機波形・規格全体適合・大規模性能測定は未実施。現在の内部エンジンと上表の実施範囲の合格を記録する。
