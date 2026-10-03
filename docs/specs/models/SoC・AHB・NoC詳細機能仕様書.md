# SoC・AHB・NoC詳細機能仕様書

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版作成。SoC共有バス・AHB相当・NoCの抽象評価契約を追加 |

文書ID：`spec-soc-models`

文書状態：契約確定。製品実装・製品実行試験は未実施。

## 共有基盤と三つの評価単位

SoC共有バス、AHB相当バス、NoCは1.0.0以後の将来対象であり、以下の三つの親要件を別profileとして具体化する。共通入力・ps時刻・phase・結果包絡を第1節で規定し、資源の単位と進行条件を各モデル節で規定する。

| 親要件 | 構成・入力→競合・転送の確認 | 結果と親要件の受入 |
| --- | --- | --- |
| [SoC共有バス](../../要件定義書.md#dir-req-0186) | [soc](#soc)でsource/targetとアドレス領域を解決し、有限source容量、FIFO、共有Bus仲裁、byte量とservice cycleによる完了をつなぐ | 一つのBus占有と要求のOKAY/ERROR、待ち・完了量・未完了を[DIR-AC-0045](../../要件定義書.md#dir-ac-0045)で照合する |
| [AHB相当バス](../../要件定義書.md#dir-req-0190) | [ahb](#ahb)で接続と4byte要求を検証し、Manager間仲裁、address/data段階、waitとERROR追加cycleをつなぐ | ERRORもprotocol完了として資源を解放することと、全段階の占有を[DIR-AC-0046](../../要件定義書.md#dir-ac-0046)で照合する |
| [NoC](../../要件定義書.md#dir-req-0194) | [noc](#noc)でmesh/XY経路を確定し、source FIFOから各Router入力FIFO・下流slot予約・出力ごとの仲裁へ進める | hop別転送と最終local配送を追い、backpressure・停止・deadlock分類を[DIR-AC-0047](../../要件定義書.md#dir-ac-0047)で照合する |

SoC/AHBの一要求はglobal active一件の共有Busを使い、NoCの一packetは複数hopを経る。NoCでは最初のgrant後の次hop待ちもactiveに含め、途中hop完了を最終配送量に足さない。比較する待ち・遅延・利用率は第1.1節の母数で求める。これらはデータbyte値を保存するAXIやメモリprofileと異なり、要求量と応答・配送の抽象評価である。

## 1. 共通入力・配置・結果

| 項目 | 確定契約 |
| --- | --- |
| 評価目的 | 複数送信元の共有資源競合、遅延、スループット、有限容量による破棄とbackpressureを、再現可能な簡潔なtransactionモデルで比較する |
| 共通設定 | INIは`model-profile`、`model-config`、`workload`、`network`、`ned-path`、`sim-time-limit`、`metrics-window`を[設定仕様](../設定詳細機能仕様書.md)に従って指定する。model JSONは全階層で必須キー完全一致、重複/未知キー・booleanを整数にした入力を準備失敗にする |
| 型 | I=指数/小数を含まないJSON整数。時間は共通時間文字列からu64 psへ厳密変換する。P=`clock_period`は正整数ps。容量・cycle数は1～65535、例外の0可を明記する。pathは配置後の正規module path、IDはASCII識別子。配列は本文で指定した順序以外をpath順に正規化する |
| 負荷 | root=`{schema_version:2,generators:[...]}`。generator=`{id,kind,node,times,transaction}`、全キー必須。id全体一意、node送信元path、times非減少非負時間文字列配列、空・重複時刻可。kindはSoC=`soc.explicit.v1`、AHB=`ahb.explicit.v1`、NoC=`noc.explicit.v1`。request_id=id+":"+0始まりordinal。T以後を含め全入力を検証し、生成はt<Tだけ(time,id,ordinal)順に行う |
| DES位相 | phase0完了/解放→phase1生成→phase2仲裁。同じtで既存完了が容量を解放してから生成を受理する。候補時刻はceil(t/P)*P。新たに空いた資源を同edge phase2で再選出できる。各転送は1cycle以上経過して完了する |
| 接続 | 各profileの登録moduleはNEDパラメータ集合空。protocolは各profile名、messageはRequest/Response（NoCはPacket）、payload schema名はprotocol+"."+message、版は1とする。配置の全moduleをconfigが一回ずつ参照し、全ポートの型/方向/接続先を検証する。直接辺又はdelay=0のFixedDelayを使用する |
| 拡張 | profile名とschema版でdispatchする。別仲裁・QoS・仮想チャネル・新protocolを別profileに登録し、既存契約を保持する |
| エラー | 入力・配置不整合E-0001、event codec不正E-0002 invalid_event、復号後状態不一致E-0002 model_failed、u64時刻/連番あふれE-0004。transactionのERRORは正常なモデル応答として扱う |
| 停止 | Tのeventは未処理。正常H=T、異常Hは共通の確定prefix。生成済み台帳はpending/active/completed/droppedの相互排他状態を持ち、generated=4状態の和。未完了転送の予定を保持し、finishは状態snapshot後に資源を解放する |
| 注記 | 信号・電気・RTLの適合、データ値のread-after-write、CPU命令実行、キャッシュ一貫性は本3profileの対象外。AHB相当モデルはAXI仕様を置換しない。 |

### 1.1. 共通結果契約

| 項目 | 確定契約 |
| --- | --- |
| wrapper | [結果仕様](../結果詳細機能仕様書.md)のschema2 ModelRecordを使用。全field必須、origin_request_id=null。以下Dは非負canonical十進文字列、Sは文字列、?はnull可。schema_version=1 |
| transaction | schema_name=`soc.transaction` / `ahb.transaction` / `noc.transaction`。record_id=request_id、subject=送信元、time_ps=最終commit時刻。data=`{source:S,target:S?,operation:S?,address:D?,bytes:D,generated_ps:D,status:S,start_ps:D?,completed_ps:D?,response:S?,drop_reason:S?,active_plan:{resource:S,hop:D,start_ps:D,planned_end_ps:D}?}`。targetはSoC/AHB decode失敗時及び未grant時だけnull。operation/addressはSoC/AHBで要求値、NoCではnull。startは最初の資源grant時、responseは完了時のみOKAY/ERROR、drop_reasonはdroppedのみsource_full |
| transfer | schema_name=`soc.transfer` / `ahb.transfer` / `noc.transfer`。record_id=request_id+":"+0始まりhop、subject=資源path、request_idを設定。data=`{hop:D,from:S,to:S?,start_ps:D,end_ps:D,bytes:D,response:S?}`。完了した転送だけtime_ps=endで追記。SoC/AHBはhop0だけ、NoCは最終local配送を含む。NoC responseは最終配送時のみOKAY、他はnull |
| 指標の有限集合 | prefix p=soc/ahb/nocごと`p.generated,p.completed,p.dropped,p.pending,p.active,p.errors`はcount/integer/summary/sum、`p.queue_mean`はcount/number/summary/time_mean、`p.queue_max`はcount/integer/summary/max、`p.wait_mean_ps,p.latency_mean_ps`はps/number/summary/sample_mean、`p.utilization`は1/number/window_summary/occupancy_ratio、`p.delivered_bits`はbit/integer/window_summary/sum、`p.throughput_bps`はbit/s/number/window_summary/rate。versionは文字列1、descriptor13個をID辞書順に登録する |
| target・集約 | 件数・待ち/遅延平均・delivered_bits/throughputのtargetは送信元及び$all、queueはSoC/AHB送信元又はNoC入力queue ID、utilizationはbus又はNoC出力resource ID。receiver/reasonはnull、request_id=null。sample_countはsample_meanのみD、他null。窓系を全窓とsummaryへ、残りsummaryだけに出す。点行は0件 |
| 定義 | errors=completedかつERROR。wait=start−generated、latency=completed−generated。対応イベント確定済みだけを平均に含む。delivered_bitsはOKAY完了要求の8*bytes、throughput=bits*10^12/区間長。NoCの途中hop量はdelivered_bitsへ加算しない。busyは各grant～完了の半開区間和。queueは転送中/予約slotを除く待機件数 |
| 積分とゼロ | 初期queue0、生成/開始/完了ごとに整数積分と最大を更新し、最後の値をHまで延長する。窓/丸め/失敗prefixは共通結果契約に従う。H=0で件数・最大・bit量0、率・平均null。H>0の空負荷はqueue/利用率/率0、標本平均null、sample_count=0。prepare失敗は結果行空 |


<a id="soc"></a>

```trace
{
  "id": "spec-soc-models#soc",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0186",
    "DIR-REQ-0187",
    "DIR-REQ-0188",
    "DIR-REQ-0189"
  ],
  "upstream": [
    "DIR-FUNC-0044"
  ],
  "state": "confirmed",
  "pending": []
}
```

## 2. SoC共有バス `soc.shared.v1`

| 項目 | 確定契約 |
| --- | --- |
| 登録・NED | `dir.soc.Source`（output request,input response）、`dir.soc.Bus`（input request_SUFFIX,output response_SUFFIXをsourceごと、output request_TARGET,input response_TARGETをtargetごと）、`dir.soc.Target`（input request,output response）。Source↔Bus↔Targetの対応suffixを対にする。source/targetのNED名をsuffixとし同じbus上で一意にする |
| config | `{schema_version:1,profile:"soc.shared.v1",clock_period,arbitration,sources,bus,targets,bytes_per_cycle}`。arbitration=round_robin/fixed_priority、bytes_per_cycle=I 1～4096、busは唯一のBus path。sourcesは非空`{node,capacity,priority}`配列（priority=I 0～65535、小さい方が優先）。targetsは非空`{node,base,size,service_cycles,error_ranges}`配列 |
| target | base=I 0～2^32−1、size=I 1～2^32、base+size<=2^32、target区間は相互非重複。service_cycles=I 0～65535。error_rangesは`{start,end}`絶対byte半開区間配列、target内かつ相互非重複。配列空可 |
| transaction | `{operation,address,bytes}`、operation=read/write、address=I 0～2^32−1、bytes=I 1～65535、address+bytes<=2^32。byte値ではなく転送量を評価する |
| 受理とFIFO | 各sourceのcapacityはpending+active上限。満杯生成はdropped/source_full、受理は生成順FIFO。全バスでactive一件。target別の並列処理も同じ共有資源を使用する |
| 仲裁 | round_robinはsource path順、cursor初期0、eligible先頭を持つ最初を選択し次indexへ進める。fixed_priorityは(priority,path)最小を選択。grant後完了まで所有者保持、activeの追越しを行わない |
| 時間・応答 | g=grant edge、b=ceil(bytes/bytes_per_cycle)。全byteが一target内ならend=g+(b+service_cycles)*P。target境界跨ぎ/未decodeはend=g+b*P、target=null、ERROR。decode済みでerror_range交差なら同じservice時間後ERROR、残りOKAY。error判定もbytes全域を用いる |
| 評価 | source待ち、bus占有、target待ちを含む総遅延を共通結果へ渡す。高priority連続流による低priority待ちも観測可能とする |
| 注記 | fixed_priorityの飢餓回避は本profileの保証範囲外。データ応答は完了通知へ抽象化し、逆向き独立bus帯域はモデル化範囲外。 |


<a id="ahb"></a>

```trace
{
  "id": "spec-soc-models#ahb",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0190",
    "DIR-REQ-0191",
    "DIR-REQ-0192",
    "DIR-REQ-0193"
  ],
  "upstream": [
    "DIR-FUNC-0045"
  ],
  "state": "confirmed",
  "pending": []
}
```

## 3. AHB相当 `ahb.transaction.v1`

| 項目 | 確定契約 |
| --- | --- |
| 登録・配置 | SoCと同じポート構造を持つ`dir.ahb.Manager`,`dir.ahb.Bus`,`dir.ahb.Target`。sourcesの代わりにmanagersを使用する。単一managerをAHB-Lite相当、複数managerをプロジェクト独自共有仲裁拡張として表す |
| config | `{schema_version:1,profile:"ahb.transaction.v1",clock_period,managers,bus,targets}`。managers=`{node,capacity}`非空配列。targets=`{node,base,size,wait_cycles,error_ranges}`非空配列。base/sizeは4byte整列、その他範囲はSoCと同じ。wait_cycles=I 0～65535 |
| transaction | `{operation,address,bytes}`、operation=read/write、bytes=I固定4、address=I 0～2^32−4の4byte整列値。単一NONSEQ/SINGLE・32bit full-width相当の要求として扱う |
| 仲裁・容量 | manager path順round_robinと生成順FIFO、pending+active容量はSoC規則と同じ。global active一件を保持する |
| 段階 | grant gからaddress段階[g,g+P)、続いてdata段階。target内ならwait_cycles回のHREADY相当待ちを挿入し、OKAY完了=g+(2+wait_cycles)*P。ERRORではerror応答の追加1cycleを含めg+(3+wait_cycles)*Pに完了する。未decodeはwait=0、target=nullとしてERROR |
| ERROR | 全byteがtarget内でerror_range非交差ならOKAY、その他ERROR。error-range交差はdecode済targetのwaitを適用する。ERRORは一件の完了として容量を解放する |
| 計測 | busyはaddress・data・wait・error追加cycle全域を含む。転送量はOKAY完了の4byte。manager競合待ちはwait_mean_ps、data待ちは総latencyと占有に反映する |
| 注記 | AHB-Liteは単一managerのinterfaceである。複数manager仲裁・直列化は本モデル独自の評価前提であり、AHBのアドレス/data pipeline重畳、burst、SPLIT/RETRY、lock、pin waveformの適合を主張しない。Armのaddress/dataとwait/error概念を参照し、時刻式は本profileで明示した抽象契約を正本とする。 |


<a id="noc"></a>

```trace
{
  "id": "spec-soc-models#noc",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0194",
    "DIR-REQ-0195",
    "DIR-REQ-0196",
    "DIR-REQ-0197"
  ],
  "upstream": [
    "DIR-FUNC-0046"
  ],
  "state": "confirmed",
  "pending": []
}
```

## 4. NoC `noc.xy.v1`

| 項目 | 確定契約 |
| --- | --- |
| 登録 | `dir.noc.Endpoint`（output packet,input delivery）、`dir.noc.Router`（input local_in,output local_out及び実在隣接方向d=east/west/north/southごとのinput in_d,output out_d）。隣接routerの反対方向へ一対一接続し、各routerにEndpoint一つをlocal接続する |
| config | `{schema_version:1,profile:"noc.xy.v1",clock_period,columns,rows,bytes_per_cycle,link_cycles,input_capacity,source_capacity,routers,endpoints}`。columns/rows=I 1～32、bytes_per_cycle=I 1～4096、link_cycles=I 0～65535、capacityは1～65535。routers=`{node,x,y}`、endpoints=`{node,router}`配列。全格子座標を一回ずつ含み各routerにendpoint一つ |
| transaction | `{destination,bytes}`。destinationは登録Endpoint path、bytes=I 1～65535。送信元と同じdestinationも有効。各packetが占めるslotはbytesによらず1、slot当たり最大65535byteを確保する |
| XY | xをdestination xへ単調に進め、等しくなったらyへ単調に進め、到着routerはlocal_outへ送る。east=x+1,north=y+1。meshはwrapのない矩形。hop数はマンハッタン距離+local配送1 |
| queue | 各Routerにlocal_inと実在する各in_dの独立FIFOを持ち、容量はinput_capacity。Endpointにcapacity=source_capacityの生成順FIFOを置く。生成時にsource FIFOが満杯ならdropped/source_full。acceptedはpendingとなる |
| 注入順 | 各edge phase2の各走査冒頭にEndpoint path順でlocal_inへ空き数まで移す。source FIFO内でeligibleな先頭だけ注入する。移したpacketは同edgeの出力候補となる。source FIFO待ちとlocal_in待ちはpending、最初の出力grantでactiveとなり、以降queue待ち中もactiveを維持する |
| 出力調停 | 出力ごと独立非preemptive。router path→出力名UTF-8辞書順で処理し、入力名UTF-8辞書順round_robin（cursor初期0）でheadの次hopが当出力かつ下流に空きslotがある最初のFIFOを選ぶ。head以外の追越しを行わない。勝者の次入力indexへcursorを進める |
| slotとbackpressure | 下流空き=input_capacity−queue件数−予約slot数。grantでsource headを取り除き、下流slotを直ちに予約し、source slotを解放する。完了で予約を解除し下流FIFOへappendする。満杯は現在queueに保持し、完了・注入・slot解放で調停を再評価する。local_outのEndpoint受信は常時受理、予約slotは不要 |
| 時間 | 各出力の占有長は(ceil(bytes/bytes_per_cycle)+link_cycles)*P。store-and-forwardで全packet到着後のedgeで次hopを開始できる。同じedge phase2でslot解放により再試行が必要なら同phaseの再走査を行い、注入とgrantがともに0件となるまで有限回反復する。busy outputは再grant不可 |
| 競合再現 | 下流入力ごとのFIFO appendはphase0の(resource ID,request_id)順。予約はphase2の上記走査順。並列出力は独立に占有し、双方向リンクも別資源。routerに全体一件の制限を置く処理は対象外（注） |
| 生存性 | XYの単調次元順と方向別入力queue、常時受理する終点によって循環channel依存を避ける。未完了packetがあり、調停固定点で進行中出力0、future生成/eligible wake0ならmodel_failed/deadlockとして状態を確定prefixまで出力する。有限T打切りはdeadlockと分類しない |
| 資源ID | 出力=`router_path:out_east`等又は`:local_out`、queue=`router_path:in_west`等又は`:local_in`、source queue=`endpoint_path:source`。transferのfrom/toはrouter又は最終Endpoint path。transaction targetはdestination Endpoint |
| 注記 | wormhole/flit、VC、adaptive routing、torus、fault routing、endpoint応答によるprotocol deadlockは本profile対象外。フリット単位の忠実度ではなく有限slotのstore-and-forward評価を行う。 |

## 5. 根拠と検証

[Arm IHI0033B.b AHB/AHB-Lite](https://documentation-service.arm.com/static/5f91607cf86e16515cdc3b4b)のaddress/data・HREADYの概念を参照（2026-09-30公開検索索引確認、本文再取得は制限あり）。[大学講義のdimension-order routing説明](https://pages.cs.wisc.edu/~tvrdik/8/html/Section8.html)にあるmeshの次元順を参考とし、本書では単調XY・方向別queue・常時受理終点という条件を具体化する。配置、容量、時刻式、結果schemaはプロジェクト独自の確定契約。

[詳細設計](../../design/SoC・AHB・NoC詳細設計書.md)、[検証仕様](../../verification/cases/SoC・AHB・NoC検証仕様書.md)、[完全fixture](../../verification/fixtures/soc/scenarios.json)を参照する。
