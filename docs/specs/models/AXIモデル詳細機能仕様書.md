# AXIモデル詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 9ad16c4`
予定公開版：`v1.1.3`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-05` | 本書の初期抽象profileの組込み実装・製品照合とv1.1.3向け提供範囲を記録 |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：AXI4 transaction profileの入力・五チャネル・RAM・応答・結果を確定 |

文書ID：`spec-axi-models`

| 項目 | 内容 |
| --- | --- |
| 文書状態 | 契約確定。開発中ソースへ組込み実装を追加し、独立fixtureとの製品実行照合を実施 |

組込み実装の入口は`input`／`runtime`／`output`のモデル別アダプターである。実施した製品試験と内部codec・停止境界の範囲は[検証記録](../../verification/cases/AXIモデル検証仕様書.md#product-execution)を参照する。公開Registry/Envelope拡張APIへの適合と規格全体の適合は、この製品実行照合の主張に含めない。

## 親要件とtransactionの確認順序

[AXI読書き評価](../../要件定義書.md#dir-req-0133)は構成・要求入力、容量と競合、五チャネル、RAM内容、応答、打切りを合わせた評価能力を表す。組込み初期profileをv1.1.3向けPRで提供する。本書の契約確定はリリース完了を示さない。

| 確認段階 | 担当する契約 | 結果をつなぐ観点 |
| --- | --- | --- |
| 構成と要求の受理 | [profile](#profile)・[transactions](#transactions) | Manager・Interconnect・Ramの接続とクロックを確定し、aligned INCR、4KiB境界、ストローブとManager容量を検査する。入力不正と生成時outstanding_fullを区別する |
| 選出とチャネル進行 | [channels](#channels) | Manager内FIFOとManager間round-robinからgrantを決め、AW/W/B又はAR/RのVALID標本・READY待ち・handshakeを独立のedge列で照合する |
| データ・応答・終了 | [memory](#memory)・[records](#records) | commit済みWSTRB byteとread snapshotをRAM行へ対応付け、B/RLAST到達でcompletedを確定する。共通journalと[0,T)から未完了・部分データを保存する |

writeの一部WがT未満に到達しBがT以上にある場合、RAM更新済みbyteは保持され、transactionはactiveである。応答がDECERR/SLVERRでもB/RLASTまで到達すればcompletedとして容量を解放するため、正常protocol応答のerrorと実行失敗を別に確認する。`completed=okay+slverr+decerr`とRAM byte内容の両方がこの経過と一致する必要がある。

親要件の確認は入力・容量の[DIR-AC-0031](../../要件定義書.md#dir-ac-0031)、チャネル・終了の[DIR-AC-0032](../../要件定義書.md#dir-ac-0032)、メモリ・再現性の[DIR-AC-0033](../../要件定義書.md#dir-ac-0033)を合わせる。ps時刻、phase、診断、schema2包絡は共通基盤の分担で、五チャネル依存・byte順・応答選択はAXI固有の分担である。

## 1. プロファイル・配置と登録

<a id="profile"></a>

```trace
{
  "id": "spec-axi-models#profile",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0133",
    "DIR-REQ-0134",
    "DIR-REQ-0144"
  ],
  "upstream": [
    "DIR-FUNC-0031",
    "DIR-FUNC-0032",
    "DIR-FUNC-0033"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 採用 | `axi4.transaction.v1`。Manager一つ以上、Interconnect一つ、Ram一つの32bitメモリマップAXI4 transaction評価。固定単一クロック、ID=0、aligned full-width INCRを扱う |
| 忠実度 | 五チャネルのhandshakeとbackpressureをクロックedgeで表現する。Interconnect全体でactive transaction一件という直列資源を採用。注：独立チャネルの並行転送、複数ID/out-of-order、FIXED/WRAP、narrow/unaligned、exclusive/atomic、QoS、cache/protection、副クロック、動的reset、pin waveform、電気・RTL適合は現profileの対象外 |
| 登録 | `dir.axi.Manager`、`dir.axi.Interconnect`、`dir.axi.Ram`をModuleRegistryへ登録する。NEDパラメータ集合は空。値はmodel-configで指定する。protocol=`axi4.transaction.v1`、message=`Aw`,`W`,`B`,`Ar`,`R`、payload schema版1 |
| Managerポート | output aw,w,ar / input b,rのscalar5個。Interconnectの同じsuffixのinput aw_SUFFIX,w_SUFFIX,ar_SUFFIX / output b_SUFFIX,r_SUFFIXへ接続する |
| RAM側ポート | Interconnect output aw,w,ar / input b,rとRam input aw,w,ar / output b,rを対応づける。各Managerの5経路を同じsuffixへ結び、全suffixがManager一つと対になることを検証する |
| 接続前提 | 同一profileの型と版を照合し、compound包装を受理する。経路は直接辺又はdelay=0のFixedDelayのみ受理する。注：正のchannel delay、Manager↔Ram直結、複数Interconnect/Ram、他deviceとの混成は本profile対象外 |
| INI | `[General] model-profile = "axi4.transaction.v1"`、`model-config = "model.json"`、`workload = "workload.json"`を使う。network/ned-path/T/出力の共通規則は[設定仕様](../設定詳細機能仕様書.md)と[操作仕様](../インタフェース詳細機能仕様書.md)を適用 |
| model-config root | 必須キー`schema_version`=整数1、`profile`=本profile、`clock_period`=正整数psへ換算可能な時間文字列、`managers`=非空配列、`interconnect`=正規path、`ram`=下記object。全階層で未知・欠落・重複キーと型違反を準備失敗にする |
| Manager設定 | `{node,max_outstanding,b_ready,r_ready}`。nodeは配置されたManagerの一意path。max_outstandingは整数1～256、readyは後述pattern。配列は配置Manager全件を一回ずつ含む |
| Ram設定 | `{node,base,size,initial,aw_ready,w_ready,ar_ready,read_latency_cycles,write_response_cycles,error_ranges}`。baseは4byte整列0～2^32−4、sizeは4の倍数4～65536、base+size<=2^32。latencyはいずれも整数1～65535cycle。nodeは唯一のRam path |
| RAM初期値 | initialは`{offset,data}`の配列。offsetは0以上整数、dataは非空偶数桁hex、offset+byte数<=size。重なる区間を準備失敗とする。残りbyteは0。error_rangesはmemory節を正本とする |
| 拡張 | profile/schema名と版でdispatchする。将来並行ID、routing、複数RAM、異なるburstやclockを新profileで登録し、同じ共通event/結果wrapperを使う。既存profileの順序・時間・応答意味を保持する |

## 2. 要求と容量

<a id="transactions"></a>

```trace
{
  "id": "spec-axi-models#transactions",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0135",
    "DIR-REQ-0140",
    "DIR-REQ-0143"
  ],
  "upstream": [
    "DIR-FUNC-0031",
    "DIR-FUNC-0032",
    "DIR-FUNC-0033"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 負荷root | `{schema_version:2,generators:[...]}`。空配列可。generator必須`id,kind,node,times,transaction`。kind=`axi.explicit.v1`、idはASCII識別子で全体一意、nodeはManager path。timesは非減少の非負時間文字列配列、同時刻重複可 |
| transaction | 共通必須`operation`=read/write、`address`=0～4294967295のJSON整数、`beats`=1～256のJSON整数。writeのみ必須`write_data`=各8桁hex文字列のbeats個配列、`write_strobes`=各0～15整数のbeats個配列。readは共通3キーのみ。boolean、小数・指数token、未知キーを拒否する |
| AXI field | addressは4byte整列、address+4*beats<=2^32、floor(address/4096)=floor((address+4*beats−1)/4096)を要求。AxLEN=beats−1、AxSIZE=2、AxBURST=INCR(1)、AxID=0を導出する。注：利用者のfield上書きは対象外 |
| 識別と生成 | request_id=`generator_id:ordinal`、ordinalはtimesの0始まりindex。生成順は(time,generator ID,ordinal)。T未満だけ生成し、T以後の入力も妥当性検査する。全時刻・容量・件数の算術を検査する |
| 候補時刻 | P=clock_period、eligible_ps=ceil(generated_ps/P)*P（checked u64）。generated時刻にpendingとなり、eligible以後のedgeで選出できる。idleかつ次候補がfuture edgeならそのedgeの仲裁機会を予約する |
| 容量 | max_outstandingはManager内のpending+active件数。生成時に使用量<上限ならpending、満杯ならdropped/outstanding_full。既存要求は保持。completedで使用量を一つ減らす。phase0完了と同時刻phase1生成なら減算後の容量を使う |
| 局所順序 | 各Managerは生成順FIFOで候補を出す。round-robinはManager間の選択だけを行う。全体でread/writeを一つのFIFO順にし、responseが完了した後に次のtransactionを開始する |
| 停止 | 状態はpending→active→completed又は生成時dropped。completedはOKAYとerror応答の両方を含む。generated=completed+dropped+pending+active。Tにある生成/handshakeは未処理として残す |

## 3. クロック・調停・五チャネル

<a id="channels"></a>

```trace
{
  "id": "spec-axi-models#channels",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0136",
    "DIR-REQ-0139",
    "DIR-REQ-0141"
  ],
  "upstream": [
    "DIR-FUNC-0032"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| clock | 立上りedge nはn*P、nは非負整数。生成はphase1、既存handshakeはphase0、選出はphase2。1回のedgeで同じtransactionの二段階を進める処理は対象外（注）。次段階の最早edgeは少なくともn+1 |
| round-robin | Managerを正規path順に整列しcursor初期0。idleのedgeでcursorから巡回してeligibleなFIFO先頭を持つ最初のManagerを選ぶ。grant時cursorを勝者の次indexへ進め、response完了まで所有者を保持する |
| READY pattern | 各ready文字列は1～1024個の0/1、少なくとも一個1。edge nでpattern[n % length]をREADY値とする。aw/w/arはRam、b/rは選出Managerの設定を使う。Wの実効READYはpattern値とAW受理済み条件のAND（write行参照）。pattern位相の原点は実行時刻0で固定 |
| handshake | 最早VALID標本edge e以後で実効READY=1の最小edge hでtransferを一回確定する。VALIDのassertionとpayload確定はeより前のcycleに行ったと抽象化し、eからhまで同じ値を保持する。valid_since_ps=e*Pを最初の標本edgeとして記録する。READY待ちによってVALID開始を遅らせる処理は対象外（注） |
| write | grant=edge g、AWとW beat0はともに最早VALID標本g+1。W0のVALID/payloadはAWREADYから独立に確定する。RamはAW handshake aの次edge a+1以後に限りWREADYをpattern値で立てるのでW0 handshakeはa+1以後。W beat i+1は前beat handshake+1からVALID。WLASTはi=beats−1だけtrue。最終W handshake w後、B最早w+write_response_cycles。B handshakeでcompleted、資源を解放する |
| read | grant=edge g、AR最早g+1。AR handshake a後、R beat0最早a+read_latency_cycles。次Rは前beat handshake+1。RLASTは最終beatだけtrue、最終R handshakeでcompleted、資源解放する |
| 再選出 | response完了phase0と同じedgeのphase2で次要求を選出できる。新要求の最初のAW/ARは翌edge以後。仲裁0候補ならidleを維持する |
| 共有リンク | Interconnectは透明な同一clockのend-to-end経路として扱い、一つのAW/W/B/AR/R transferにhandshakeを一件記録する。注：2hopを独立に加算する遅延や二重handshake記録は対象外 |
| 占有・計測 | resourceはInterconnect path。busyは[grant,completed)、queue使用量はManagerのpending件数、outstandingはpending+active。共通計測の時間積分へ状態変化を渡す。チャネルの待ちcycle=(handshake_ps−valid_since_ps)/Pをhandshake記録から求める |

## 4. RAM・データ・応答

<a id="memory"></a>

```trace
{
  "id": "spec-axi-models#memory",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0137",
    "DIR-REQ-0138",
    "DIR-REQ-0142"
  ],
  "upstream": [
    "DIR-FUNC-0031",
    "DIR-FUNC-0033"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| byte順 | write_data/read_dataの8桁hexは昇順アドレスの4byteを表す。例11223344はmemory[A:A+4]=11,22,33,44で、32bit数値は0x44332211。WSTRB bitjはdataのj番目byte（WDATA[8j+7:8j]）を有効にする |
| write副作用 | beat iのアドレスはA+4*i。OKAYなら各W handshakeのcommitでWSTRB=1のbyteだけ更新し、0のbyteは保持する。WSTRB=0も一beatとして消費する。注：burst全体のatomic書込を仮定する処理は対象外 |
| read値 | OKAYなら各R beatが最初にVALIDになる段階で対象4byteをsnapshotし、READY待ち中は保持する。本profileはglobal直列のため他transactionからの途中書換えは発生しない。先行transactionのW commitを後続readへ反映する |
| error_ranges | `{start,end,access}`配列、start/endは絶対byteアドレスでbase<=start<end<=base+size、accessはread/write/both。半開区間を使用し、どの二範囲も重複を準備失敗にする。空配列可 |
| response選択 | AW/AR handshakeでburst全byte領域を判定する。RAM範囲外byteを含めばDECERR、それ以外でaccessが適合するerror_rangeと一byte以上交差すればSLVERR、残りはOKAY。WSTRB=0のbyteもアドレス領域判定に含める |
| errorの完了 | write errorでも全Wをhandshake後Bで応答し、RAM更新は0byte。read errorはbeats個の00000000データと同じerror responseを返し、最終Rで完了する。error応答は正常なprotocol完了でありシミュレータ実行失敗とは区別する |
| 打切り | 途中Wまでcommit済みのbyteはT終了でもRAMに保持する。B未到達はactive。readの一部Rだけ到達した場合は到達済みdataのみtransaction.read_dataへ保持する。未到達beatの合成は対象外（注） |

## 5. payload・記録と検証参照

<a id="records"></a>

```trace
{
  "id": "spec-axi-models#records",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0133",
    "DIR-REQ-0143",
    "DIR-REQ-0144"
  ],
  "upstream": [
    "DIR-FUNC-0031",
    "DIR-FUNC-0032",
    "DIR-FUNC-0033"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定契約 |
| --- | --- |
| 外部payload | schema=`axi4.transaction.v1.`+Aw/W/B/Ar/R、版1。全objectは未知/欠落/重複キーを拒否し、キーをUTF-8辞書順に整列したcompact UTF-8 JSON bytesで運ぶ。Dは非負canonical十進文字列、Sは文字列 |
| Aw / Ar | `{request_id:S,manager:S,target:S,address:D,len:D,size:2,burst:1,id:0}`。len=beats−1、addressは要求の先頭。Envelopeのpath/時刻/型を対応状態と照合する |
| W | `{request_id:S,beat:D,data_hex:S,strb:D,last:boolean}`。data_hexは8桁小文字、strb=0～15、beat=0始まり。lastは最終beatでtrue |
| B / R | B=`{request_id:S,id:0,resp:S}`、R=`{request_id:S,id:0,beat:D,data_hex:S,resp:S,last:boolean}`。respはOKAY/SLVERR/DECERRの一つ。profileでfield意味を確定する |
| 記録wrapper | [結果仕様](../結果詳細機能仕様書.md)schema2のModelRecordを使う。schema_name/schema_version/record_id/subject/request_id/origin_request_id/time_ps/dataは全必須。純AXIのorigin_request_idはnull。以下dataの全キー必須、?はnull可 |
| axi.transaction | schema_version1。record_id=request_id、subject=Manager。data=`{manager:S,interconnect:S,target:S,operation:S,address:D,beats:D,generated_ps:D,eligible_ps:D,status:S,grant_ps:D?,completed_ps:D?,response:S?,read_data:S[],drop_reason:S?}`。statusはpending/active/completed/dropped。responseはAW/ARの決定後に設定、drop_reasonはdroppedだけoutstanding_full。time_psは最後のcommit遷移時刻 |
| axi.handshake | schema_version1。record_id=`request_id:channel:beat`（AW/AR/Bのbeat suffixは0）、subject=Interconnect、request_idはtransaction。data=`{channel:S,beat:D?,valid_since_ps:D,address:D?,data_hex:S?,wstrb:D?,last:boolean?,response:S?}`。time_psはhandshake edge。AW/ARはaddressだけ、Wはbeat/data/wstrb/last、Bはresponseだけ、Rはbeat/data/last/responseだけを値とし他はnull |
| axi.memory | schema_version1。record_id=subject=Ram path、request_id=null。data=`{base:D,size:D,data_hex:S}`は全RAM byte列を昇順アドレスの小文字hexで保存。time_psは最後の有効WSTRB付きW commit（初期0）、値が同じでも有効byteを書けば更新する |
| 記録の存在 | transactionは生成commit、handshakeは該当edgeのcommit、memoryはinitialize全成功後に現れる。停止や失敗は共通journalの確定prefixから出力する。共通requests/receiversへのAXI行混入は対象外（注） |
| 検証 | [AXI詳細設計](../../design/AXIモデル詳細設計書.md)、[AXI検証仕様](../../verification/cases/AXIモデル検証仕様書.md)、[完全fixture](../../verification/fixtures/axi/scenarios.json)へ接続する。実装試験は未実施 |


### 5.1. AXI指標の有限登録集合

| metric_id | unit | value_kind | sampling | aggregation |
| --- | --- | --- | --- | --- |
| `axi.generated`, `axi.completed`, `axi.dropped`, `axi.pending`, `axi.active`, `axi.okay`, `axi.slverr`, `axi.decerr` | count | integer | summary | sum |
| `axi.queue_length`, `axi.outstanding` | count | integer | point | identity |
| `axi.queue_mean`, `axi.outstanding_mean` | count | number | summary | time_mean |
| `axi.queue_max`, `axi.outstanding_max` | count | integer | summary | max |
| `axi.bus_utilization` | 1 | number | window_summary | occupancy_ratio |
| `axi.wait_ps`, `axi.latency_ps` | ps | integer | point | identity |
| `axi.wait_mean_ps`, `axi.latency_mean_ps` | ps | number | summary | sample_mean |
| `axi.read_bits`, `axi.written_bits` | bit | integer | window_summary | sum |
| `axi.read_throughput_bps`, `axi.written_throughput_bps` | bit/s | number | window_summary | rate |
| `axi.channel_stall_cycles` | count | integer | point | identity |

| 項目 | 確定契約 |
| --- | --- |
| descriptor | 全24指標のversionは文字列`1`。metadata.metricsは上表をmetric_id単位に展開した`{metric_id,version,unit,value_kind,sampling,aggregation}`をUTF-8辞書順で保存する。[metrics.json](../../verification/fixtures/axi/metrics.json)が完全descriptor例。注：純AXI実行へのCAN37指標や未登録IDの追加は対象外 |
| Record形式 | 共通Recordの既存fieldだけを使う。点はtime_ps/event_seq/effect_seqを設定しstart_ps/end_ps=null、summaryと窓はstart_ps/end_psを設定しtime_ps/event_seq/effect_seq=null。request_idは要求起因の点だけ、receiverは全指標null。reasonはdropped集計のoutstanding_fullとchannel_stall_cyclesのチャネル名だけ、それ以外null。sample_countはsample_meanだけ件数D、それ以外null |
| 件数 | targetは各Manager、Interconnect、$all。generatedは生成済transaction数、completed/dropped/pending/activeは終端台帳のstatus件数。okay/slverr/decerrはcompletedのうちresponseが一致する件数。activeで応答判定済みの要求は応答件数の対象外（注）。各targetに8指標を0件でもsummary出力する。generated=completed+dropped+pending+active、completed=okay+slverr+decerr |
| queue/outstanding | targetは各Manager path。Q=pending数、O=pending+active数。初期値0とGenerate判定後（dropも含む）、grant後、B/RLAST完了後にqueue_lengthとoutstandingの点をこの順で出す。grantでQだけ減少、完了でOだけ減少。H>0の初期点はrequest_id/event_seq/effect_seq=null。各値を次遷移時刻まで保持する |
| 時間平均と最大 | queue_mean=∫Qdt/H、outstanding_mean=∫Odt/H。queue_max/outstanding_maxは初期値と全commit後の最大を取り、時刻0だけの一時peakも含む。各Managerに4指標のsummaryを出す。注：この4指標の窓行は対象外 |
| busy | targetはInterconnect。bus_utilization=measure(union[grant,completed)∩区間)/区間長。activeの占有はHまでclipし、通常の完了後のidleもHまで積分し、異常時は確定prefixのHで打ち切る。各窓とsummaryに一行ずつ出す |
| 待ち・遅延 | grantでwait_ps=grant−generated、B/RLASTでlatency_ps=completed−generatedの点をtarget=Managerへ出す。waitはclock整列とFIFO/仲裁待ちを含む。wait_mean_ps/latency_mean_psは各Manager、Interconnect、$allに標本数を付けてsummary出力。未grant/未完了は該当平均の標本対象外（注） |
| 転送量 | targetはInterconnect。read_bitsはOKAYの各R handshakeで32bit、written_bitsはOKAYの各W handshakeで8*popcount(WSTRB)bitを計上。error応答のbeatとWSTRB0の書込量は0。停止時もcommit済みbeatだけを含める。各窓とsummaryにbit量を出し、read_throughput_bps/written_throughput_bpsは対応bit量*10^12/区間長を出す |
| チャネル待ち | 各handshakeでchannel_stall_cycles=(handshake_ps−valid_since_ps)/Pをtarget=Interconnect、request_id=当該要求、reason=AW/W/B/AR/Rで一回出す。0cycleも記録する。W0ではAW待ちによるRam側WREADY抑制も含む。注：チャネル別平均・窓合算の追加指標は対象外 |
| 同一callbackの点順 | Generate/grantはqueue_length→outstanding→wait_ps（grant時だけ）。handshakeはchannel_stall_cycles→queue_length→outstanding→latency_ps（後3件はB/RLAST時だけ）。初期点はManager path順。各成功commitの呼出し順をeffect_seqへ反映する |
| 窓とsummary | H/W/半開区間、最終短縮窓、最終状態更新からHまでの積分、異常prefixのH時刻点、整数演算とbinary64丸めは[共通結果集計](../結果詳細機能仕様書.md#aggregation)を適用する。各Interconnectに5個のwindow_summary指標を全窓とsummaryへ出す。その他summary指標は[0,H)一行、point指標はrecordsだけに出す |
| 空集合・ゼロ | 正常H=0はpoint/窓0行、件数・bit量・最大0、平均/使用率/throughput=null、sample_meanのsample_count=0。H>0の空負荷は初期gauge0、時間平均/使用率/bit量/throughput0、標本平均null。drop0もreason=outstanding_fullで出す。prepare失敗は共通規則どおりrecords/summary空。異常H=0は確定済み点・件数・最大を保持し、0分母の平均/率はnull |

## 6. 参照と根拠の範囲

| 項目 | 確定契約 |
| --- | --- |
| Arm原典 | [IHI0022H,2020,Part A](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/IHI0022H_amba_axi_protocol_spec.pdf)のAXI4構成とfieldを参照。AXI4はAW/W/B/AR/Rを持ち、burst長・byte strobeを運ぶ。本文や図を転載せずプロジェクト契約を記述する |
| Arm公開解説 | [Introduction to AMBA AXI4,102202,2020](https://developer.arm.com/-/media/Arm%20Developer%20Community/PDF/Learn%20the%20Architecture/102202_0100_01_Introduction_to_AMBA_AXI.pdf)のhandshake、channel依存、response、strobeを確認。READY/VALIDが成立する立上りで転送、応答はwrite data後という原則を採用 |
| 境界確認 | [Arm IHI0022J,2023,A4.1](https://documentation-service.arm.com/static/63ff0ebd56ea36189d4e7ee7)で4KiB境界とINCRのアドレス進行を確認。現行最新版全体への準拠主張は対象外（注） |
| プロジェクト決定 | global一件直列、ready pattern、latency、FIFO/round-robin、容量破棄、RAM error-range、JSON形状と結果は本profile独自の評価前提。2026-09-29資料確認、RTL/実機/conformance testの合格主張は対象外（注） |
