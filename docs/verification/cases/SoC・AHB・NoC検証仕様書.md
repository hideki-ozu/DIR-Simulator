# SoC・AHB・NoC検証仕様書

文書バージョン：`0.1.0`
対象GitHubバージョン：`未リリース（main @ 7738b55）`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版作成。完全入力と解析期待値6ケースを追加 |

文書ID：`verification-soc-models`

文書状態：契約確定。解析fixture照合済み。製品実行試験は未実施。

| 項目 | 確定契約 |
| --- | --- |
| 対象 | [機能仕様](../../specs/models/SoC・AHB・NoC詳細機能仕様書.md)、[詳細設計](../../design/SoC・AHB・NoC詳細設計書.md)の3profileと各共有資源 |
| 完全入力 | [scenarios.json](../fixtures/soc/scenarios.json)の6ケースごとにINI、model/workload JSON、models/ケース名/Main.nedを保存する。時刻の単位はps。相対パスは各INIのあるfixtureディレクトリを基準に解決する |
| 解析確認 | rootから`python3 docs/verification/fixtures/soc/verify_fixtures.py`。参照存在、profile整合、要求ID、生成件数保存、時刻式、byte量、XY経路、出力占有の非重複とbackpressure解析時刻を固定期待値と照合する |
| 製品実行計画 | `dir-simulator run --config docs/verification/fixtures/soc/soc-round-robin.ini --output /tmp/dir-soc-round-robin`。他ケースも対応INIと未使用出力先へ置換する。製品CLI/イベントループの実行は未実施 |
| 製品照合 | ModelRecordのtransactionからrequest_id,start_ps,completed_ps,status,response,active_planを取り出し、Dを整数に変換してexpected.transactionsへ投影する。transferはrequest_id,subject,start_ps,end_psをexpected.hopsと比較する。transaction行順はID順、hop行は(request_id,hop)順へ正規化して比較する |
| metrics照合 | $all delivered_bitsはexpected.delivered_bitsへ完全一致。busのutilizationはbusy_ps/Tを共通規則でbinary64へ一度丸める。窓も半開区間へbusyをclipして照合する。NoCは各outputのtransferと未完了予定から同様に占有を算出する |
| 境界・不正追加計画 | schema未知/重複キー、bool整数、負容量、範囲重複、未配置path、AHB非整列、mesh欠落/重複座標、不正隣接、時刻overflowを個別に与え、prepare不正E-0001と実行overflow E-0004を確認する。製品不正入力試験は未実施 |
| 注記 | checkerは独立した算術・資料照合であり、製品のqueue実装、scheduler、NED parser、protocol適合の合格証拠ではない。 |

<a id="dir-test-0044"></a>

## 1. SoC round-robinと共有帯域

```trace
{
  "id": "DIR-TEST-0044",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0186",
    "DIR-REQ-0187",
    "DIR-REQ-0188",
    "DIR-REQ-0189"
  ],
  "upstream": [
    "design-soc-models#soc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 対応 | DIR-FUNC-0044、[DIR-AC-0045](../../要件定義書.md#dir-ac-0045)。同familyのroot・子要件を一緒に照合する |
| 入力 | [soc-round-robin.ini](../fixtures/soc/soc-round-robin.ini)。同時刻0にm0の8byte二件とm1の8byte一件。幅4byte/cycle、service1cycle、P10ps、capacity2。 |
| 解析期待 | grant順a:0,b:0,a:1、開始0/30/60ps、完了30/60/90ps。T100ps、busy90ps、delivered192bit。 |
| 判定 | transaction状態・開始/完了時刻・応答・転送量を整数で完全一致させる。停止時のactiveと破棄を分離し、容量・占有の不変条件を各commitで確認する |
| 実施状態 | fixture checkerの解析値照合済み。製品実行・commitごとの内部不変条件試験は未実施 |

<a id="dir-test-0045"></a>

## 2. SoC容量・ERROR・停止

```trace
{
  "id": "DIR-TEST-0045",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0186",
    "DIR-REQ-0187",
    "DIR-REQ-0188",
    "DIR-REQ-0189"
  ],
  "upstream": [
    "design-soc-models#soc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 対応 | DIR-FUNC-0044、[DIR-AC-0045](../../要件定義書.md#dir-ac-0045)。同familyのroot・子要件を一緒に照合する |
| 入力 | [soc-capacity-stop.ini](../fixtures/soc/soc-capacity-stop.ini)。m0 capacity1、error範囲16～20、4byte要求を0/0/20psに生成。T40ps。 |
| 解析期待 | a:0は20ps ERROR完了、a:1はsource_full、20psの完了解放後にa:2を受理・開始。40psの完了は未処理でactive、active_plan={resource:Main.bus,hop:0,start_ps:20,planned_end_ps:40}、busy40ps、delivered0。 |
| 判定 | transaction状態・開始/完了時刻・応答・転送量を整数で完全一致させる。停止時のactiveと破棄を分離し、容量・占有の不変条件を各commitで確認する |
| 実施状態 | fixture checkerの解析値照合済み。製品実行・commitごとの内部不変条件試験は未実施 |

<a id="dir-test-0046"></a>

## 3. AHB waitとerror追加cycle

```trace
{
  "id": "DIR-TEST-0046",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0190",
    "DIR-REQ-0191",
    "DIR-REQ-0192",
    "DIR-REQ-0193"
  ],
  "upstream": [
    "design-soc-models#ahb"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 対応 | DIR-FUNC-0045、[DIR-AC-0046](../../要件定義書.md#dir-ac-0046)。同familyのroot・子要件を一緒に照合する |
| 入力 | [ahb-wait-error.ini](../fixtures/soc/ahb-wait-error.ini)。m0正常・m1 error-rangeへ4byte、wait2、P10ps、同時刻0生成。 |
| 解析期待 | a:0開始0・完了40ps、b:0開始40・ERROR完了90ps。busy90/100、delivered32bit。 |
| 判定 | transaction状態・開始/完了時刻・応答・転送量を整数で完全一致させる。停止時のactiveと破棄を分離し、容量・占有の不変条件を各commitで確認する |
| 実施状態 | fixture checkerの解析値照合済み。製品実行・commitごとの内部不変条件試験は未実施 |

<a id="dir-test-0047"></a>

## 4. AHB decode失敗と停止境界

```trace
{
  "id": "DIR-TEST-0047",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0190",
    "DIR-REQ-0191",
    "DIR-REQ-0192",
    "DIR-REQ-0193"
  ],
  "upstream": [
    "design-soc-models#ahb"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 対応 | DIR-FUNC-0045、[DIR-AC-0046](../../要件定義書.md#dir-ac-0046)。同familyのroot・子要件を一緒に照合する |
| 入力 | [ahb-boundary.ini](../fixtures/soc/ahb-boundary.ini)。m0 capacity1、target0～32へaddress64の要求二件を同時刻0。T30ps。 |
| 解析期待 | a:0はdecodeERRORの予定30psを持つactive、active_plan={resource:Main.bus,hop:0,start_ps:0,planned_end_ps:30}。a:1はdropped。transfer完了0件、busy30ps、delivered0。 |
| 判定 | transaction状態・開始/完了時刻・応答・転送量を整数で完全一致させる。停止時のactiveと破棄を分離し、容量・占有の不変条件を各commitで確認する |
| 実施状態 | fixture checkerの解析値照合済み。製品実行・commitごとの内部不変条件試験は未実施 |

<a id="dir-test-0048"></a>

## 5. NoC XYと並列資源定義

```trace
{
  "id": "DIR-TEST-0048",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0194",
    "DIR-REQ-0195",
    "DIR-REQ-0196",
    "DIR-REQ-0197"
  ],
  "upstream": [
    "design-soc-models#noc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 対応 | DIR-FUNC-0046、[DIR-AC-0047](../../要件定義書.md#dir-ac-0047)。同familyのroot・子要件を一緒に照合する |
| 入力 | [noc-xy.ini](../fixtures/soc/noc-xy.ini)。2×2 mesh、e00→e11の8byte、幅4、link0、P10ps。 |
| 解析期待 | r00 east0～20、r10 north20～40、r11 local40～60ps。Xの後にY、3hop、T70psでcompleted、delivered64bit。 |
| 判定 | transaction状態・開始/完了時刻・応答・転送量を整数で完全一致させる。停止時のactiveと破棄を分離し、容量・占有の不変条件を各commitで確認する |
| 実施状態 | fixture checkerの解析値照合済み。製品実行・commitごとの内部不変条件試験は未実施 |

<a id="dir-test-0049"></a>

## 6. NoC有限容量・backpressure・停止

```trace
{
  "id": "DIR-TEST-0049",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0194",
    "DIR-REQ-0195",
    "DIR-REQ-0196",
    "DIR-REQ-0197"
  ],
  "upstream": [
    "design-soc-models#noc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 対応 | DIR-FUNC-0046、[DIR-AC-0047](../../要件定義書.md#dir-ac-0047)。同familyのroot・子要件を一緒に照合する |
| 入力 | [noc-backpressure.ini](../fixtures/soc/noc-backpressure.ini)。2×1、input/source capacity1。e10自身へ16byteを0ps、e00→e10の4byteを0/10/20/20ps。T65ps。 |
| 解析期待 | b:0がlocalを0～40占有。a:0はwest入力で10～40待ち完了50、a:1は下流満杯で10～40待ち開始40・完了60。a:2開始50・local60～70の途中でactive、active_plan={resource:Main.r10:local_out,hop:1,start_ps:60,planned_end_ps:70}、a:3はsource_full。generated5=completed3+active1+dropped1、delivered192bit。 |
| 判定 | transaction状態・開始/完了時刻・応答・転送量を整数で完全一致させる。停止時のactiveと破棄を分離し、容量・占有の不変条件を各commitで確認する |
| 実施状態 | fixture checkerの解析値照合済み。製品実行・commitごとの内部不変条件試験は未実施 |
