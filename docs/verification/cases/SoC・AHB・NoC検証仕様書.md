# SoC・AHB・NoC検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 9ad16c4`
予定公開版：`v1.1.3`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-05` | 本書の独立fixtureと製品出力の照合、入力・codec・停止prefix・Viewer試験と実行記録を追加 |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版作成。完全入力と解析期待値6ケースを追加 |

文書ID：`verification-soc-models`

文書状態：契約確定。解析fixtureと開発中ソースの製品実行結果を照合済み。内部試験・追加境界の実施範囲は末尾の検証記録を参照する。

| 項目 | 確定契約 |
| --- | --- |
| 対象 | [機能仕様](../../specs/models/SoC・AHB・NoC詳細機能仕様書.md)、[詳細設計](../../design/SoC・AHB・NoC詳細設計書.md)の3profileと各共有資源 |
| 完全入力 | [scenarios.json](../fixtures/soc/scenarios.json)の6ケースごとにINI、model/workload JSON、models/ケース名/demo/Main.nedを保存する。時刻の単位はps。相対パスは各INIのあるfixtureディレクトリを基準に解決する |
| 解析確認 | rootから`python3 docs/verification/fixtures/soc/verify_fixtures.py`。参照存在、profile整合、要求ID、生成件数保存、時刻式、byte量、XY経路、出力占有の非重複とbackpressure解析時刻を固定期待値と照合する |
| 製品実行計画 | `dir-simulator run --config docs/verification/fixtures/soc/soc-round-robin.ini --output /tmp/dir-soc-round-robin`。他ケースも対応INIと未使用出力先へ置換する。製品CLI/イベントループを実行済み（末尾参照） |
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
| 実施状態 | fixture checkerの解析値と製品出力を照合済み。commit・rollbackの内部検査の実施範囲は末尾の製品検証記録を参照 |

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
| 実施状態 | fixture checkerの解析値と製品出力を照合済み。commit・rollbackの内部検査の実施範囲は末尾の製品検証記録を参照 |

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
| 実施状態 | fixture checkerの解析値と製品出力を照合済み。commit・rollbackの内部検査の実施範囲は末尾の製品検証記録を参照 |

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
| 実施状態 | fixture checkerの解析値と製品出力を照合済み。commit・rollbackの内部検査の実施範囲は末尾の製品検証記録を参照 |

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
| 実施状態 | fixture checkerの解析値と製品出力を照合済み。commit・rollbackの内部検査の実施範囲は末尾の製品検証記録を参照 |

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
| 実施状態 | fixture checkerの解析値と製品出力を照合済み。commit・rollbackの内部検査の実施範囲は末尾の製品検証記録を参照 |

## 追加ケースの再現と実施境界

以下は保存済みfixtureを変更せず、実装時に一時ディレクトリへ複製して表の差分を適用する検証入力の仕様である。既存INIのnetwork/profile/clock/窓幅と無変更値を継承し、相対model/workload/NED参照を複製先にそろえる。生成順は(time,id,ordinal)、全時刻はps。追加入力の製品実行・内部状態注入試験は未実施であり、既存fixture checkerの合格件数へ加算しない。期待値は仕様の整数式と半開区間から独立に求める。

<a id="dir-test-0090"></a>

## 7. SoC固定優先度・同順位・非preemptive

```trace
{
  "id": "DIR-TEST-0090",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0188"
  ],
  "upstream": [
    "design-soc-models#soc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 入力と期待結果 |
| --- | --- |
| 基準 | DIR-TEST-0044の完全入力を複製し、arbitrationだけfixed_priority、m0.priority=1、m1.priority=0へ変更。T100、他値同じ |
| 固定優先度 | b:0,a:0,a:1を開始0/30/60、完了30/60/90で処理。全応答OKAY。priority昇順をpath順より先に適用し、同一sourceのa:0がa:1に先行する |
| 同順位 | m0.priority=m1.priority=0に変更するとpath小のm0を毎回選ぶためa:0,a:1,b:0、開始0/30/60、完了30/60/90。RRのa:0,b:0,a:1とは異なる |
| 後着 | 基準の固定優先度でa.times=[0]、b.times=[10]に変更。a:0は0～30を保持し、優先度の高いb:0は30～60。bのwait=20、latency=50。10psにactiveの所有者又は予定完了を置換しない |
| 内部判定 | grant直前のeligible FIFO head集合、(priority,path)勝者、FIFO pop前後、capacity=pending+activeを採取する。fixed_priorityはRR cursorを選択根拠にしない。時刻・順・stateをjournalと照合する |
| 受入・状態 | DIR-AC-0045。仕様・設計照合と独立期待値算定済み。製品実行・状態採取は未実施 |

<a id="dir-test-0091"></a>

## 8. SoC複数target・gap・境界跨ぎ

```trace
{
  "id": "DIR-TEST-0091",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0187",
    "DIR-REQ-0189"
  ],
  "upstream": [
    "design-soc-models#soc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 入力と期待結果 |
| --- | --- |
| 構成 | DIR-TEST-0044を複製。m0だけから生成、capacity=8、m1は配置/configに残すが生成0。targets=[{node:Main.t,base:0,size:16,service_cycles:1,error_ranges:[]},{node:Main.t1,base:32,size:16,service_cycles:3,error_ranges:[]}]。NEDにt1:demo.Target、Busのoutput request_t1/input response_t1、bus.request_t1→t1.request、t1.response→bus.response_t1を追加。P10、幅4、T150 |
| 負荷 | generatorsは全件kind=soc.explicit.v1,node=Main.m0,times=[0ps],operation=read。id/address/bytesをa/0/8、b/32/8、c/20/6、d/12/8、e/44/8とする。他generatorは削除 |
| 正常 | a:0はtarget=Main.t、開始0・完了30、b:0はMain.t1、開始30・完了80。duration=(2+1)*10と(2+3)*10。異なるtargetでもglobal active一件で直列 |
| 未decode | c:0はgap内20～26、target=null、ERROR、開始80・完了100。ceil(6/4)*10=20であり任意targetのserviceを加えない |
| 境界 | d:0の12～20とe:0の44～52はそれぞれtarget終端を跨ぐ。target=null、ERROR、開始100/120・完了120/140。始点だけのdecodeを採用しない |
| 集計 | generated=completed=5、errors=3、pending=active=dropped=0、delivered_bits=128。busy=140、utilization=14/15。wait標本[0,30,80,100,120]のmean66、latency標本[30,80,100,120,140]のmean94、各sample_count=5 |
| 内部判定 | 要求全byte半開区間、decode対象、ceil beat数、service加算分、active_planと最終transfer.toを採取する。ERRORも完了・容量解放・遅延標本に一回含め、配送bit量へ含めない |
| 受入・状態 | DIR-AC-0045。独立期待値算定済み。追加配置のNED parser・製品実行は未実施 |

<a id="dir-test-0092"></a>

## 9. AHB単一managerと全段階の停止

```trace
{
  "id": "DIR-TEST-0092",
  "stage": "verification",
  "requirements": [
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

| 項目 | 入力と期待結果 |
| --- | --- |
| 完全入力差分 | DIR-TEST-0046を複製しconfig.managersはMain.m0一件capacity2、workloadはa一件times=[0ps],operation=read,address=0,bytes=4。NEDのm1 submodule、Bus.request_m1/response_m1とその二接続を削除。target Main.tのwait_cycles=2、error_rangesは基準を継承。P10 |
| 内部段階 | grant0、address_end10、nominal data終了20、waitを含むdata_end40、正常予定完了40。address[0,10)、data[10,20)、wait[20,40)を保持し、外部イベントを段階tickごとに増やさない |
| 途中停止 | T=5/15/25ではそれぞれaddress/data/wait内。すべてstatus=active,start=0,completed=response=null,active_plan={resource:Main.bus,hop:0,start_ps:0,planned_end_ps:40}。generated=active=1、completed=0、transfer0件、delivered_bits=0、busy=5/15/25、summary utilization=1 |
| 等号境界 | T40でもCompleteは未処理でactive・busy40・bits0。T41はcompleted/OKAY、completed_ps40、active_plan=null、transfer一件、delivered32、busy40、utilization40/41。wait_mean0/sample_count1、latency_mean40/sample_count1 |
| ERROR段階 | addressを16に変更するとdecode済ERRORでplanned_end50、error段階[40,50)。T45とT50ではactive・completed=null・busy45/50・bits0、T51でcompleted/ERROR、errors1、busy50、delivered0、latency50/sample_count1。targetのwait2を保持する |
| 窓 | W10、T25の窓は[0,10),[10,20),[20,25)でbusy10/10/5、各utilization1。全段階の実区間をHでclipし、予定40/50までの未来busyを加えない |
| 内部判定 | 単一managerのcursor/FIFO、active段階境界、最終timer、busy積分、finish前snapshotを採取。未完了をcompleted又はtransferへ補完せず、capacityとactiveを停止snapshot後に解放する |
| 受入・状態 | DIR-AC-0046。独立期待値算定済み。段階観測・製品停止試験は未実施 |

<a id="dir-test-0093"></a>

## 10. NoC同一出力RR・cursor・FIFO先頭

```trace
{
  "id": "DIR-TEST-0093",
  "stage": "verification",
  "requirements": [
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

| 項目 | 入力と期待結果 |
| --- | --- |
| 完全入力差分 | DIR-TEST-0048の2×2配置を継承。input_capacity=2,source_capacity=4,P10,幅4,link0,T80。全generator times=[0ps]。a:Main.e00→Main.e10,4byte、b:Main.e10→Main.e10,16byte、c:Main.e10→Main.e10,4byte、d:Main.e10→Main.e00,4byte。id順a,b,c,dで注入、既存workloadを置換 |
| 前半 | r00:out_eastがa:0を0～10、r10:local_outがb:0を0～40。r10:local_inにc:0,d:0の順、in_westにa:0が10psから待つ。dの要求出力out_westはidleでも、先頭cの次hopがlocal_outの間はdを取り出せない |
| 競合 | 40psでlocal_outの候補はin_west先頭a:0とlocal_in先頭c:0の二件で、ともにeligible。r10入力名はin_north,in_west,local_in。bのlocal_in勝利後cursorは0。40psはin_westのa:0を選び40～50、cursorはlocal_inのindex2へ |
| 次のgrant | 50psにc:0をlocal_outで50～60、cursorは0へ。cをpopした後、同edge phase2でd:0をout_westへ50～60、r00:local_outへ60～70。dの開始は50であり0～40のidle westを使う追越しは発生しない |
| 台帳 | a/b/c/dのfirst start=0/0/50/50、final completed=50/40/60/70、全OKAY。generated=completed=4、delivered224bit。途中hopの4byteを重複加算しない |
| 内部判定 | 各grant時の出力別cursor前後、二候補のhead/request/eligible/slot、local FIFO[c,d]、同phase再走査を採取。勝者以外のheadは保存し、busy localと独立out_westの同時grantを許す |
| 受入・状態 | DIR-AC-0047。独立経路・時刻算定済み。製品RR/cursor/FIFO試験は未実施 |

<a id="dir-test-0094"></a>

## 11. NoC異なる出力と双方向の並列

```trace
{
  "id": "DIR-TEST-0094",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0195",
    "DIR-REQ-0197"
  ],
  "upstream": [
    "design-soc-models#noc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 入力と期待結果 |
| --- | --- |
| 完全入力差分 | DIR-TEST-0048の2×2配置、input_capacity=source_capacity=2、P10、幅4、link0、T30。全件4byte,times=[0ps]。a:Main.e00→Main.e10、b:Main.e00→Main.e01、c:Main.e10→Main.e00、既存workloadを置換 |
| 並列 | r00:out_eastのa:0、r00:out_northのb:0、r10:out_westのc:0がすべて0～10を占有。r00はaをFIFOからpop後にbを同edgeでgrantする。単一routerの二出力と一physical linkの反対方向を独立資源として扱う |
| 配送 | 10～20にr10/r01/r00の各local_outへ配送、全request first start0、completed20、2hop。delivered96bit、各使用出力busy10/T30=1/3。r00 eastとr10 westの区間重複を違反と判定しない |
| 内部判定 | outputごとのactive一件、3件の下流slot予約と10psの予約解除/appendを採取。同時刻Complete cohortを(resource ID,request_id)順に一batchでcommitし、phase2までに全slotを反映する |
| 受入・状態 | DIR-AC-0047。独立期待値算定済み。製品並列出力・cohort試験は未実施 |

<a id="dir-test-0095"></a>

## 12. NoC進行固定点・future Wake・deadlock・T優先

```trace
{
  "id": "DIR-TEST-0095",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0197"
  ],
  "upstream": [
    "design-soc-models#noc"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 入力と期待結果 |
| --- | --- |
| 検証方法 | MeshContextの固定点後progress判定を内部状態注入で検証する。公開入力は単調XYで循環依存を避けるため、不正meshや架空の合法deadlock負荷を新たな受理入力として定義しない。注入stateは実装試験専用で、prepare成功の証拠に使わない |
| 共通snapshot | 正常2×1配置のcontext、P10、判定時刻20、T100。journalに生成済p:0一件pending、source FIFOにp:0、active全null、future生成なし。検証用注入により当該固定点で注入/grantをともに0へ固定する。queue/台帳/capacityの保存条件を満たし、commit済journal prefix Jと予約表を記録する |
| 進行不能 | future eligibleなし、timerなしの場合はE-0002、reason=model_failed、details.operation=deadlockを出す。status=execution_failed,partial=true。失敗遷移は非commit、exportはJの確定prefixのみで、pはpendingのまま、completed/transfer/deliveredを増やさない |
| 起床あり | 上記のeligibleだけ30へ変更。判定20でdeadlockを出さずWake一件time30を保持。再判定で二重予約しない。早いeligible25を別headへ与えたときはedge ceil(25/10)*10=30を使う。eligible21も同じWake30へ合成する |
| 生成あり | eligibleなしでもfuture Generate30がある場合はその生成を待ち、deadlockを出さない。既存GenerateをWakeに複製しない。通常の生成・phase2経路を進行条件とする |
| T優先 | 判定時刻T20に到達した条件では共通停止を先に適用し、正常time_limit、H20、pending1、E-0002なし。Tのphase2/注入/progressを実行しない。pendingをdeadlock理由で改変しない |
| 内部判定 | 未完了数、active出力数、future生成/eligible最小値、Wake所有表、診断details、Jのbyte列を採取。失敗batchからtimer・journal・metrics deltaが漏れないことを比較する |
| 受入・状態 | DIR-AC-0047。仕様・設計の診断分類照合済み。注入試験・製品失敗prefix試験は未実施 |

<a id="dir-test-0096"></a>

## 13. 三profileの全13指標・母数・ゼロ・窓

```trace
{
  "id": "DIR-TEST-0096",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0189",
    "DIR-REQ-0193",
    "DIR-REQ-0197"
  ],
  "upstream": [
    "design-soc-models#soc",
    "design-soc-models#ahb",
    "design-soc-models#noc"
  ],
  "state": "confirmed",
  "pending": []
}
```

各profileのprefixをpとする。13 descriptorをID辞書順、version="1"で完全一致させ、未登録指標や点行を追加しない。以下のcountとbitsはcanonical十進文字列、numberは正確有理数から共通規則で一度binary64へ丸める。source/$all、queue、資源以外のtargetを混ぜない。sample_countはwait/latencyだけ、request_id/receiver/reasonは全行null。

| 指標（p.） | DIR-TEST-0044 SoC・H100 | DIR-TEST-0046 AHB・H100 | DIR-TEST-0048 NoC・H70 |
| --- | --- | --- | --- |
| generated | $all3、m0=2,m1=1 | $all2、m0=1,m1=1 | $all1、e00=1、他source0 |
| completed | $all3、m0=2,m1=1 | $all2、m0=1,m1=1 | $all1、e00=1、他0 |
| dropped | 全source/$all0 | 全source/$all0 | 全source/$all0 |
| pending | 全source/$all0 | 全source/$all0 | 全source/$all0 |
| active | 全source/$all0 | 全source/$all0 | 全source/$all0 |
| errors | 全source/$all0 | $all1,m0=0,m1=1 | 全source/$all0 |
| queue_mean | m0=60/100,m1=30/100 | m0=0,m1=40/100 | 全source/input queue0。予約slotとactiveを除く |
| queue_max | m0=2,m1=1（生成直後・grant前も観測） | m0=1,m1=1 | e00:source,r00:local_in,r10:in_west,r11:in_south各1、他queue0 |
| wait_mean_ps | $all30/n3,m0=30/n2,m1=30/n1 | $all20/n2,m0=0/n1,m1=40/n1 | $all/e00=0/n1、他source null/n0 |
| latency_mean_ps | $all60/n3,m0=60/n2,m1=60/n1 | $all65/n2,m0=40/n1,m1=90/n1 | $all/e00=60/n1、他source null/n0 |
| utilization | Main.bus=90/100 | Main.bus=90/100 | r00:out_east,r10:out_north,r11:local_out各20/70、他output0 |
| delivered_bits | $all192,m0=128,m1=64 | $all32,m0=32,m1=0 | $all/e00=64、他source0 |
| throughput_bps | $all=192*10^12/100,m0=128*10^12/100,m1=64*10^12/100 | $all/m0=32*10^12/100,m1=0 | $all/e00=64*10^12/70、他source0 |

| 追加母数・境界 | 必須の期待結果 |
| --- | --- |
| schema照合 | 件数6個=count/integer/summary/sum、queue_mean=count/number/summary/time_mean、queue_max=count/integer/summary/max、wait/latency=ps/number/summary/sample_mean、utilization=1/number/window_summary/occupancy_ratio、delivered=bit/integer/window_summary/sum、throughput=bit/s/number/window_summary/rate。窓3種は各窓とsummary、他10種はsummaryだけ |
| 独立母数 | SoC0044のstart−generated標本[0,30,60]、completed−generated[30,60,90]。AHB0046は[0,40]と[40,90]でERRORも両標本へ含む。NoC0048は最初のgrantだけwait標本、最後のlocal Completeだけlatency標本であり、3hopをsample_count3にしない |
| ERRORと未完了 | SoC0045のH40はgenerated3=completed1+active1+dropped1、errors1、bits0。wait0/n2（activeにも開始実績あり）、latency20/n1。完了応答ERRORだけerrorsを増す。AHB0092のT25はwait0/n1、latency null/n0、queue_mean0でactive1をqueueへ加えない |
| reservedとqueue | NoC0048のr10:in_westは[0,20)でreserved1,Q0、20psにappend後同edge grant。queue_mean0、queue_max1、capacity検査はQ+reserved。NoCのsource FIFO、各input FIFOを別queue対象として出力する |
| NoC途中停止 | 0048をT25に変更。active1,completed0,bits0,throughput0、wait0/n1、latency null/n0。r00 east busy20/25、r10 north busy5/25、r11 local0。最初のhop transfer8byteがあってもdelivered_bits0 |
| H0 | 三基準fixtureをT0へ変更。全count/queue_max/delivered0、queue_mean/wait/latency/utilization/throughput=null、wait/latency sample_count0、窓0件。台帳とtransfer空、全descriptorは存在 |
| H正・空負荷 | 各workload.generators=[]、T100。全count/queue_max/delivered0、queue_mean/utilization/throughput0、wait/latency=null,n0。各登録source/queue/outputに0又はnullを明示し、存在しない受信対象を作らない |
| 半開窓 | SoC0044をW30へ変更。busy=[30,30,30,0]、bits=[0,64,64,64]、窓長=[30,30,30,10]。完了30/60/90は次窓へ帰属。summary bits192、throughput192*10^12/100であり窓率の単純平均でない |
| 内部判定・状態 | 整数queue積分、busy区間、標本分子/数、台帳状態、descriptor/target集合を採取してjournal prefixから再集計。独立期待値算定済み、製品集計・export試験未実施。DIR-AC-0045/0046/0047 |

<a id="product-execution"></a>

## 組込みモデルの製品検証記録

[実行記録](../results/soc-memory-product-2026-10-05.json)は開発中ソースのbinary/source hash、実行した構成、終了状態、model schema・metric集合、検証コマンドを保持する。公開タグの証跡ではなく、通常の文書版・履歴はpush準備時に確定する。

| 対象 | 製品確認 |
| --- | --- |
| 独立期待値 | `crates/dir-simulator/tests/soc.rs`が6ケースの実シミュレータ出力を時刻・状態・byte又は転送量へ対応付ける。資料だけを計算するfixture checkerとは別に実行する |
| 境界・不正 | 同ファイルと`runtime/soc/protocol.rs`の試験で容量・停止・型・未知キー・codec・時間あふれと確定prefixを検査する。具体的な実施数と対象は実行記録へ保存する |
| 結果表示 | `view`で単独HTMLを生成し、`tests/transaction_viewer_model.test.cjs`で整数時刻・未完了・前後ステップ、`tests/transaction_viewer_browser.cjs`で実結果の読込と操作を確認する |
| 証拠の境界 | 規定した抽象profileの製品試験であり、公開Registry/Envelope API全体、規格全体への適合、実機動作、100万要求の性能を合格とするものではない |

```bash
cargo test --locked -p dir-simulator --test soc
```

現在の試験範囲は本節と実行記録で判定し、個別に実施していない内部注入・追加入力の手順まで合格へ読み替えない。
