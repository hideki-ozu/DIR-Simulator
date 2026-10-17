# 実行制御・指標・checkpoint実装設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.6`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-17` | 協調制御、Metric V2、process越しcheckpointの要件・全状態・API・所有権・保存復元・独立検証条件を定義。v1.1.6収録版を確定 |

文書ID：`proposal-control-metric-checkpoint-implementation`
文書状態：承認済み限定scopeの現行実装済み（v1.1.6収録）。本書は採用した詳細契約。製品試験の実施範囲・最終判定は別記録。

[要件の入口](../後続拡張要件定義書.md)、[共通仕様](../後続拡張契約提案仕様書.md)、[アーキテクチャ](../後続拡張アーキテクチャ設計書.md)を前提とする。設計上の責務名と現在の公開宣言は区別する。正確なAPIは現行利用境界のAPI表とソースを参照する。

## process越しcheckpointの利用イメージ

長い計算を途中で保存し、シミュレーターを終了した後に、新しく起動したprocessで続きから実行するための機能である。保存済みの成功イベントを再実行せず、同時刻の未処理イベントも元の順に継続する。全実体codecを備えた汎用registered engine向けに実装した。network checkpointはUnsupportedのままとする。

| 操作 | 保持するもの | process終了後に続行 |
| --- | --- | --- |
| Pause／Resume | メモリ内の実行状態とprocess内token | 不可 |
| 結果Export | 成功済みの結果・計測・診断を読むsnapshot | 不可 |
| Checkpoint／Restore | 入力、FES、モデル／channel状態、次ID、計測途中値、journalの版付き保存 | 保存成功したbundleから同一binaryで復元可能 |

利用順は、対応能力を有効化してprepare→実行→成功境界でPause→checkpoint保存完了→process終了→同一buildでrestore→新tokenでResume。restoreの照合で破損・入力差・実装版差があれば、実行を始めず診断する。保存前の電源断から自動回復する機能ではなく、直前に保存が完了した地点まで戻れる。

初期対象は汎用registered engineと、明示codecを備える全Model／Channel。同一binary bytes・入力・codec版を要求する。network partition／既存専用engine、異版移行、外部process・通信状態の保存は初期対象外。保存と復元には実行状態を読み書きする時間・ディスク容量と各モデルのcodec実装が必要である。自動定期保存は初期版では行わず、呼出側がPauseと保存を要求する。具体的な能力・API・bundle・失敗時動作はP-CHECKPOINT-R01以降を正とする。

## 用語と初期対応範囲

**概要の機能仕様：** 同じprocess内での静止・再開、独自の整数計測、対応モデルの別processへの引継ぎを、それぞれ明示して選択する。

**概要の動作仕様：** 成功commit境界でのみ静止し、exportは確定journalの読取view、checkpointは再開に必要な全可変状態を保存する。結果JSONを読んでengineを推測復元しない。

| 対象backend | 協調制御 | Metric V2 | 永続checkpoint |
| --- | --- | --- | --- |
| 汎用registered Model／Channel | 対応（明示capability必須） | V2 catalogを選ぶprofileで対応 | 全実体に明示codecがある場合だけ対応 |
| staged NetworkState（dynamic／TSN／変換と後続network） | 対応（明示capability、checkpoint不可） | 新profileがV2を宣言した場合に対応 | 初期版は未対応。全partitionのcodecが必要 |
| 既存専用CAN／媒体／AXI／SoC／memory engines | 初期版未対応 | 既存専用指標は維持 | 初期版未対応 |

能力はprofile名から推測せずRegistryの追加能力表へ登録する。通常の同期runは能力なしでも従来どおり実行する。制御又はcheckpointを指定した実行だけを`UnsupportedCapability`で準備拒否する。能力表の未登録値は全false。network builtinは制御のみを明示し、Metric V2/checkpointの対応を推測しない。

## 要件

<a id="p-control-r01"></a>
### P-CONTROL-R01 成功境界での静止・継続

利用者はPause／Resume／Stopを明示できること。Pauseは再開可能、Stopは再開不可とし、callback途中・commit途中の状態を公開しない。初回イベント前でも静止でき、同時刻に残ったイベントを失わない。受入条件は状態表、失敗優先、H、pending、次連番の完全一致。

<a id="p-control-r02"></a>
### P-CONTROL-R02 有界で再現可能な制御

要求・ackを有限容量で管理し、再試行と同ID異内容を区別すること。再現実行はwall-clockの到着時刻ではなく、記録した成功commit ordinal・境界種別・要求順を使う。受入条件は満杯・重複・forget・ID上限・競合優先順を固定vectorで確認すること。

<a id="p-control-r03"></a>
### P-CONTROL-R03 非破壊exportと一度だけのcleanup

静止状態を破壊せず繰返しexportでき、失敗後に再試行できること。Pause／Stopそのものではfinishを呼ばず、close又は自然終端のcleanupは初期化成功済みModelとfactory成功済みChannelへ一度だけ行う。受入条件はexport前後のengine fingerprint一致、失敗後のtoken有効性、resume後の結果一致。

<a id="p-metric-r01"></a>
### P-METRIC-R01 計測定義と所有範囲

V1を維持し、V2のsampling・aggregation・対象・初期値をprepareで凍結できること。不正な組合せや未登録targetを黙殺しない。受入条件は合法組合せ表と、重複／欠落／未知field／能力不一致の拒否。

<a id="p-metric-r02"></a>
### P-METRIC-R02 正確で原子的な集計

整数pointと右連続gaugeを、成功effect順で集計すること。和・積分はchecked u128で保持し、失敗batchの一部を残さない。受入条件は同時刻複数変更、空集合、H点、窓境界、あふれの独立算術との一致。

<a id="p-metric-r03"></a>
### P-METRIC-R03 互換性と出力

旧APIと旧schemaを変えず、新しい意味は新しい出力版で表すこと。未観測の凍結targetもsummaryに現れ、平均の母数0はnullとする。受入条件はcatalog・record・summary・CSVの型、正確比率からの一回の丸め、旧版との拒否境界の一致。

<a id="p-checkpoint-r01"></a>
### P-CHECKPOINT-R01 明示capabilityによる完全状態の保存

全モデル・channelとコアに対応codecがある静止sessionだけを保存できること。部分的なserializerで成功扱いにしない。受入条件は能力不足のprepare拒否、状態集合の全件性、外部資源を持つ未対応モデルの拒否。

<a id="p-checkpoint-r02"></a>
### P-CHECKPOINT-R02 版付き・有界・原子的なbundle

版・入力・binary・Registry・全blobの対応を固定した保存物を公開すること。manifestが完全な場合だけ復元対象とし、途中保存や不一致・危険なpathを拒否する。受入条件は欠落／破損／別run混在／容量超過／公開失敗を注入して、旧bundleとlive sessionが保全されること。

<a id="p-checkpoint-r03"></a>
### P-CHECKPOINT-R03 副作用のない復元と再現

復元ではinitializeや過去イベントを再実行せず、新しいprocess内tokenで静止状態から続けられること。codec失敗は新しいarenaだけを破棄し、元bundleは変えない。受入条件は同時刻残件・取消済みtoken・乱数・積分器・spoolを含む連続実行との一致。

## 機能分担

<a id="p-control-f01"></a>
**P-CONTROL-F01：** RunSessionのownerが明示的にdriveし、制御要求でPaused／Stoppedへyieldする。Resumeは保持したengineを続ける。
<a id="p-control-f02"></a>
**P-CONTROL-F02：** Controllerは要求enqueueとack読取だけを提供する。mailbox snapshotを一つの採用境界にする。
<a id="p-control-f03"></a>
**P-CONTROL-F03：** 凍結viewからexportし、sessionのcloseがcleanupを管理する。exportの失敗はsimulation失敗へ変換しない。
<a id="p-metric-f01"></a>
**P-METRIC-F01：** RegistryでV2 descriptor・target・gauge初期値を別登録し、選択profileへ結び付ける。
<a id="p-metric-f02"></a>
**P-METRIC-F02：** Contextからpoint観測／gauge変更を提示し、AccumulatorDeltaをbatchへ含める。
<a id="p-metric-f03"></a>
**P-METRIC-F03：** 凍結viewをrecord・summary・窓へ射影し、厳密な母数と版を出す。
<a id="p-checkpoint-f01"></a>
**P-CHECKPOINT-F01：** 全実体のcodec対応を検査し、Paused状態だけをCheckpointViewへ固定する。
<a id="p-checkpoint-f02"></a>
**P-CHECKPOINT-F02：** content-addressed blobと最後のmanifestを新規保存先へ公開する。
<a id="p-checkpoint-f03"></a>
**P-CHECKPOINT-F03：** bundle・入力・実装を照合して非公開arenaへ復元し、検証成功後にだけsessionを返す。

## 詳細仕様

<a id="p-control-s01"></a>
### P-CONTROL-S01 Session・API・状態遷移

候補APIは`RunSession::new(prepared, options)`→sessionとController、`run_until_yield()`→Paused／Stopped／Completed／Failed、`resume(token)`、`export(destination)`、`close()`とする。`run_with_control(session)`はnewで作成済みsessionを値で受け取りrun_until_yieldを呼ぶ便宜入口で、全状態でsessionとstate、Paused時だけtokenを返す（Controllerはnew時に呼出側が保持する）。別sessionのControllerをpreparedへ付け替える入口は設けない。engineのownerは呼出しthread一つだけ。controllerを別threadへ渡せるが、libraryは自動worker threadを作らない。Paused中のqueued要求はownerの`poll_control()`又は次のowner操作で処理する。ownerがdrive／pollしない間にackが進む保証はない。

公開状態はReady／Initializing／Running／Paused／Exporting／Stopped／Completed／Failed／Closed。Pausingは、実行中callbackへのPause受理から成功commitまでの内部表示だけで、snapshot取得可能な状態ではない。tokenはsession ID＋generation＋prepared fingerprintへのprocess内不透明handle。process外への直列化を許さない。

| 現在状態 | Pause | Resume(valid token) | Stop | Export | Close |
| --- | --- | --- | --- | --- | --- |
| Ready | callback前にPaused、H=0、初期化未実施 | InvalidState | Stopped、H=0 | InvalidState | Closed |
| Initializing | 要求をqueueし初期化transaction完了まで保留 | InvalidState | 同左 | InvalidState | 同左。完了後Stop＋close |
| Running／Pausing | 成功境界でPaused。Stopが同境界ならSuperseded | InvalidState | 成功境界でStopped | InvalidState | Stop要求を受理し境界後close |
| Paused | Applied、状態維持、同じtoken | tokenを一回消費しRunning（未初期化ならInitializing経由） | token無効化、Stopped | Exporting→元のPaused | token無効化、cleanup後Closed |
| Exporting | Busy | Busy | Busy | Busy | Busy。完了後明示再要求 |
| Stopped | InvalidState | InvalidState | Applied、状態維持 | Exporting→Stopped | cleanup後Closed |
| Completed／Failed | InvalidState | InvalidState | Applied、元の状態維持 | Exporting→元の状態 | Closed。cleanup再実施なし |
| Closed | SessionClosed | SessionClosed | SessionClosed | SessionClosed | Applied、冪等 |

ReadyでのPause／Stopはfactory・initializeを開始しない。Initializingは全初期化効果を一括検証する既存準備transactionであり、その内部を部分Pauseしない。したがって応答境界の上限は、通常イベント中はcallback一回＋境界処理、初期化中は初期化transaction全体＋境界処理である。wall-time上限は保証しない。これは旧提案の「callback一回」を初期化へ誤適用しないための明確化である。

イベント境界では①実行済callbackの失敗→Failed、②取得済mailbox snapshot内のStop、③Pause、④自然終了、⑤次callback開始の順。失敗batchは破棄し、同境界の制御要求は`EngineFailed`をackする。Stop一件目だけをApplied、追加Stopは同終端状態を返す。Stopと競合したPause／ResumeはSuperseded、同じsnapshot内のExportは静止化後に元のFIFO位置順で実行する。snapshot後に届いた要求は次境界へ回す。自然終了候補Tで未実行ならHは直前成功時刻、正常Completedなら観測終端はT。

`RunOptions`は`max_exports`正u32（省略1024）、`checkpoint_enabled`bool（省略false）だけを初期版で受理する。mailbox容量64・同時export数1は固定値。0、未知field、未対応backend又は選択能力の不足はnew／prepareで拒否する。重い処理の実時間timeout・強制killは既存NED worker側の別機能であり、この協調APIへ混ぜない。

<a id="p-control-s02"></a>
### P-CONTROL-S02 Mailbox・ack・replay

新規submitはlibraryがmutex内でu64 request_idを0からchecked採番し、queueへ入れたときだけ消費する。checked加算で次位置を保持するため、初期版の発行値は0..u64::MAX-1、u64::MAXは発行不能sentinelとする。control_seqとboundary_seqの確定数は発行済み要求数以下に保つ。`retry(request_id, command)`は既存要求の再送専用で新IDを作らない。利用者が任意順の新IDを注入するAPIにはしない。ID・command・canonical payloadは受理後不変。

| 条件 | 結果・状態変更 |
| --- | --- |
| pending＋保持ackが64件 | Busy。IDもevent数も消費しない |
| pending同ID・同内容retry | 同じpending handle。二重実行しない |
| ack保持中の同ID・同内容retry | 同じackを返す |
| 同ID・異内容 | RequestConflict |
| forget済みID | DuplicateExpired。ID再利用なし |
| 未発行IDのretry | UnknownRequest |
| u64上限後の新規submit | ResourceExhausted。simulation状態は維持 |

ownerのresume／exportは同じsubmit＋pollの便宜操作で、既存pending要求を追い越さず、slot満杯ならBusyを返す。Closeはcleanup用のowner操作で、simulation replayとは分ける。

ackの全fieldは`request_id/control_seq/boundary_seq/boundary_kind/commit_ordinal/virtual_time_ps/state/status/export_ordinal/checkpoint_receipt/error`。整数はJSON化する場合に十進文字列、null許容はexport_ordinal／checkpoint_receipt／errorだけ。checkpoint以外のackはcheckpoint_receipt=null。statusはApplied／Pending／Superseded／InvalidState／Busy／EngineFailed／SessionClosed／ResourceExhausted。受付時のBusyやIDエラーは保存slotを取らずsubmit errorとして返す。control_seqはack確定順、commit_ordinalは成功event数（初期化は含めない）。同時刻でも別commitは区別する。

replay記録は境界種別`before_initialize/after_initialize/after_commit/paused_poll/owner_operation`、単調u64のboundary_seq、commit_ordinal、snapshot内の受付順、正規化command、ack statusを持つ。boundary_seqは非空の要求snapshot又は記録対象owner操作を取ったときだけ一回checked増分し、同じPaused／commit ordinalでの複数pollを区別する。空pollは記録を増やさない。replayはsnapshotのgroupingを維持し、複数pollを一つへ合流しない。検証用replayはその境界へ同じ要求列を注入し、到達しないordinal・異なる状態・不正token指示をReplayMismatchとして中止する。Resumeのopaque handleそのものを保存せず、同じ記録で直前に生成したpause generationを参照する。

<a id="p-control-s03"></a>
### P-CONTROL-S03 Export・結果版・cleanup

Paused／StoppedのHは最後の成功commit時刻（成功なしは0）。H時刻の成功recordとpointは全体summaryへ入り、半開窓からは除く。gauge積分は[0,H)までのviewを求め、live accumulatorのlast_time／areaを書き換えない。resume後は元の積分器から続ける。

新しい制御exportはresult schema3とし、既存schema2のmetadata／simulation構造を保持して`control`を追加する。controlの必須fieldは`version:"1"/session_id/export_ordinal/state/commit_ordinal/horizon_ps/pending_events/resume_supported/replay_sha256`。整数は十進文字列、resume_supportedはbool、replay_sha256は正規化replay列のSHA-256。ResumeTokenと認証情報は出力しない。terminationはpaused／stopped／events_exhausted／time_limit又は既存失敗理由。CSVの既存列は維持し、状態とcontrol情報はmetadata／manifest側に置く。

旧同期runのschema1/2は不変。要求なしのcontrolled実行と旧runの一致対象は仮想simulationのrecord・時刻・件数・計測・終端であり、新しいschema番号とcontrol metadataは比較から分ける。旧提案の「一致」を新APIのファイル全byte一致と解釈しない。schema3未対応Viewerは明確に拒否し、製品実装時に対応表示とvalidatorを同時追加する。

exportは新規destinationのみ、既存destinationはDestinationExists。全ファイルをstagingへ書き、hash・長さ・schemaを検査してmanifestを最後に公開する。失敗はExportFailedで元のsessionとtokenを保持し、export_ordinalは成功時だけ進む。max_exports到達はExportLimit。相対pathの基準は呼出時の明示output rootで、途中でcwdを再解釈しない。

Pause／Stopはfinishを呼ばずarenaを保持する。自然Completed／Failedは従来の逆順で一度だけfinishし、一次診断を保全する。Stopped／Pausedをcloseすると初期化成功済みModelを逆順、その後factory成功済みChannelを逆順でfinishし、戻り値CloseReportにcleanup診断を返す。close後の過去exportは書換えない。Dropはclose相当のbest effortを行いpanicを外へ出さない。callback自体が戻らない場合の強制終了は保証外である。

<a id="p-metric-s01"></a>
### P-METRIC-S01 Descriptorと観測API

候補は`register_metric_v2(descriptor, targets, initial_gauges)`、`Context::observe_point_u64(name,target,value)`、`Context::set_gauge_u64(name,target,value)`。旧`register_metric`／`observe`はV1専用のままとし、同名V1/V2を拒否する。

| descriptor／設定 | 型・単位・値域・省略 |
| --- | --- |
| name／unit | 既存MetricDescriptorの名前規則・非空unit。両方必須 |
| version | 正u32、必須。APIのV2とdescriptor版1は別 |
| value_kind | `integer`のみ、必須 |
| sampling | `point`又は`gauge`、必須 |
| aggregation | pointはidentity/sum/count/min/max/mean、gaugeはtime_meanのみ。必須、一つだけ |
| targets | 対象subjectの非空一意配列。prepareでowner modelと照合し凍結 |
| initial_gauges | gaugeは全targetにu64値を一つずつ必須、pointでは空。余分・重複・欠落を拒否 |
| max_metric_targets | 正u32、省略1024。prepareで総target対数を検査 |
| max_observations_per_batch | 正u32、省略65536。新APIの効果件数上限。0拒否 |

二つの上限はprepare前の`Registry::set_metric_v2_limits(MetricV2Limits)`で指定する。未指定なら表の既定値を使い、一般INIへ同名キーを自動追加しない。profileがJSON設定へ露出する場合はそのvalidatorが同APIへ渡すことを明記する。

Contextは自身のsubjectだけをtargetに指定できる。任意の他モデルの値は観測できず、共同指標は専用coordinatorのsubjectへ集約する。timestamp引数はなく現在時刻を使う。未登録target／誤sampling／負値・型不一致・u64超過はModelErrorとし、そのcallbackのbatch全体を棄却する。浮動入力、任意式、gaugeのmax、明示capacityとの比率式は初期対象外。

<a id="p-metric-s02"></a>
### P-METRIC-S02 演算・空集合・窓

pointは成功effect一件を一標本とし、同event内でも複数標本になる。count／sumはu128、min／maxはu64、meanはsum/countの正確有理数で保持する。identityはrecordsだけ、他はrecordsと選択summary一件。空集合のsum／countは0、min／max／meanはnull。初期gaugeは観測eventに数えない。

gaugeは`last_time/last_value/area`を保持する。tで変更すると旧値×(t-last_time)をareaへchecked加算し、その後に値を置換する。同時刻の変更幅は0、最後の成功値が次区間へ効く。Hのviewは`area + last_value*(H-last_time)`、time_meanはその値/H。H=0はnull。窓[a,b)も同じ区分積分を使い、最後の短い窓をb=Hで切る。

正常時はt<Tの成功pointを全体・対応窓へ入れる。異常／Pause／Stopの成功prefixはt<=Hを全体summaryへ入れ、窓はt<Hだけを含む。gaugeのH点変更は最終値に現れるが、[0,H)の面積に幅を与えない。あふれはpublish前にE-0004相当で止め、当該batchのrecords・summary・last_valueを全て未公開にする。

<a id="p-metric-s03"></a>
### P-METRIC-S03 出力schemaと正確値

V2を宣言するprofileはresult schema3を要求する。`metadata.metrics_v2`はname/unit/version/value_kind/sampling/aggregation/targets/initial_gaugesと解決済み上限を持つ。descriptor versionは十進文字列。V1のmetadata.metricsと混ぜず、legacyのみのprofileは従来版を保つ。

recordは既存のseq/effect_seq/time_ps/metric/target/unit/value_kind/valueにdescriptor_versionを追加する。整数値・sample_count・時刻・連番はcanonical十進文字列。summaryはmetric/target/aggregation/unit/value_kind/value/sample_count、窓にはstart_ps/end_psを追加する。countのunitはcount、mean/time_meanは元unit、sum/min/maxは元unit。gauge summaryのsample_countは成功変更数（初期値を除く）、identityにはsummaryを出さない。

sum/countはu128範囲の整数文字列、min/maxは整数又はnull、平均は既存`output/aggregate.rs::ratio`相当の正確分子・分母→binary64 ties-to-even一回丸め→有限JSON number又はnull。途中で各operandをfloatへ変換しない。内部checkpointには分子・分母又は整数stateを保存し、丸め後のnumberを集計へ戻さない。recordの決定的整列・CSV表記は既存結果仕様を引き継ぎ、V2 catalogの全targetを出す。

<a id="p-checkpoint-s01"></a>
### P-CHECKPOINT-S01 Capabilityと取得点

新しいcheckpoint bundle versionは1。初期対象は汎用registered engineだけ。`checkpoint_enabled=true`なら全Model／Channelに実装キー・実装版・codec名・正u32 codec版と`external_state:none`を宣言させる。実装コード、OS descriptor、外部process、外部通信を保存可能と推測しない。statelessも明示codec（空payload）を必要とする。

公開Model／Channelへ追加する保存hookはdefaultでUnsupportedを返す任意methodとし、既存実装へ必須関数を追加しない。Registryのcodec登録は対応実装のsnapshot hookとrestore factoryを結び付ける。restore factoryは`(config, codec_version, bytes)`から未初期化callbackを呼ばずに復元済み実体を返す。能力なし・版重複・対象漏れ・対応しないexternal_stateはprepare拒否。

codecには読取専用のCheckpointEncodeContext／DecodeContextも渡す。特にTimerTokenは非公開fieldを持つため、通常APIへowner/idのconstructorを足さず、`encode_timer_ref(token)`と`decode_timer_ref(ref)`を使う。保存表現は`{version:1,owner,id,state}`でidは十進文字列、stateはlive／retired。encode時のlive_timers集合との照合でstateを固定する。decodeはcodec対象owner一致とid<next_event_idを検査する。liveならlive_timersと有効FES予約が各一件一致し、retiredなら両者への不在を要求する。retired tokenの全履歴表は保存せず、codec内のこの参照だけを検証する。発火済み・取消済みtokenの参照も保存できるがliveへ復活させない。restore factoryの正確な入力は`(config, codec_version, bytes, DecodeContext)`、保存hookは`(&self, EncodeContext)`である。通常のscheduleを呼んでtokenを再発行する復元は禁止する。

取得はPausedだけ。ReadyPause（初期化前）も保存でき、その場合arenaは空、codec_catalogだけを持ち、codecs／codec_refs／initialized_model_ids／constructed_channel_idsは空、FES／dirty／live_timersも空である。Metric V2はprepareで得た初期値・ゼロ集計だけを保存する。復元時のfactory／restore factory／initialize呼出しは全て0、resume後に通常factory＋initializeを初めて一回実行する。初期化済Pausedでは全可変状態を保存する。Stopped／Completed／Failedの結果exportは可能だが、初期版の再開checkpointは作らない。保存中はExporting相当のBusyでresume／stop／closeと重ねない。成功してもlive tokenは消費しない。

<a id="p-checkpoint-s02"></a>
### P-CHECKPOINT-S02 Bundleの完全な構造

形式は新規directory。`manifest.json`、`state.json`、`inputs/...`、`objects/<sha256>.bin`だけを許す。ZIP展開機能は初期対象外。全JSONはUTF-8、重複key拒否、BOM拒否、最大JSON深さ64。input_fingerprintは捕捉済み全入力のlogical path・bytes・SHA-256をpath順に並べたcompact UTF-8 JSON（key辞書順、整数サイズは十進文字列、余白なし）のSHA-256、registry_fingerprintは同じ方式のdescriptor／実装版／codec／能力catalogのSHA-256。build_source_sha256とlockは既存build provenance値、binary_sha256は実行file全bytesのSHA-256。pathは相対POSIX・空要素／`.`／`..`／絶対path／symlink／hardlink別名を拒否する。

| manifest field | 型・意味 |
| --- | --- |
| schema_name／schema_version | `dir.checkpoint.bundle`／JSON整数1 |
| binary_sha256／build_source_sha256／cargo_lock_sha256／registry_fingerprint／input_fingerprint | 64文字小文字hex。復元先と完全一致を要求 |
| profile／result_schema_version | 非空profile名／JSON整数。選択版の一致を検査 |
| run_lineage／checkpoint_ordinal／commit_ordinal／horizon_ps | lineage文字列、残りは十進u64文字列 |
| state_path | 固定`state.json` |
| files | path/bytes/sha256の配列。path一意、manifest自身を除く全実在ファイルを過不足なく列挙。実ファイル集合はfilesのpath集合∪{manifest.json}。manifest自身のhashはmanifestへ格納せず、保存receiptで別途返す。bytesは十進u64文字列 |
| codec_catalog | implementation_key／implementation_version／codec_name／codec_version／external_state。全選択実装の能力を列挙 |
| codecs | model又はchannel実体ID／implementation_key／implementation_version／codec_name／codec_version／blob_path。initializedなら全実体に一件、uninitializedなら空配列 |

`state.json`の必須fieldはversion、engine_phase（uninitialized/initialized）、virtual_time_ps、last_event_time_ps（null可）、commit_ordinal、next_event_id、next_sequence、last_committed_key（最後に成功commitした(time_ps,delta,phase,sequence)、初回前null）、fes、dirty、live_timers、initialized_model_ids、constructed_channel_ids、journal、metric_accumulators、source_dispatchers、codec_refs、replay_log、replay_cursor、next_boundary_seq、next_control_id、next_control_seq、next_pause_generation。FES行は(time_ps,delta,phase,sequence)とDelivery又はArbitrationのtag、宛先、Envelope全field、TimerToken（owner/id又はnull）を保存する。dirtyはtime/delta/owner/resource、live_timersはowner/id。payloadとjournal/spoolはblob参照にできる。registered engineの外にある論理dispatcherは初期対象なら空であることを明示し、未対応backendを空値で偽装しない。

現行state.jsonは加えてconfig_path、input_mapping、input_directories（探索したNED directory一覧。空directoryを含む入力探索範囲を凍結）、cwd、options、next_export_ordinal、next_checkpoint_ordinalを保存する。last_committed_keyは復元後のFESを最後の成功順序より後へ制約し、input_directoriesは凍結入力sourceから再prepareした探索集合と完全一致を検査する。

モデル・channelごとの乱数state、保留要求、可視化用状態もcodec payloadの必須対象である。metadata catalog・入力snapshot・診断・record index・spoolの確定prefixとoffset・残りのmetric窓stateはjournalに含む。spoolは確定長までをblob化し、元processの一時pathやopen descriptorを保存しない。モデル側に乱数があればcodecがそのstateを保持し、コアがseedだけで代用しない。

limitsは`max_total_bytes`正u64（省略1073741824）、`max_files`正u32（省略100000）、`max_blob_bytes`正u64（省略268435456）、`max_fes_entries`正u32（省略1000000）。整数は設定JSONでは十進文字列、max_files/max_fes_entriesだけJSON整数。0、totalより大きいblob上限、未知fieldを保存／復元APIの操作開始前に拒否する。保存前の計数と復元前のmanifest計数の両方で検査する。

公開は同じfilesystemの新規stagingへ全blob→state→検証済manifestを書き、file sync、manifest sync、directory sync後に新規destinationへrenameし親directoryをsyncする。既存destinationを置換しない。filesystemが必要なdurable操作を提供できなければDurabilityUnsupported。rename前の失敗はlive sessionと旧bundleを保持し、作業所有のstagingだけを回収する。rename後のparent sync失敗はPublishedDurabilityUnknownのreceiptを返し、公開済bundleを保持してordinalを消費する。receiptにはdestination／manifest_sha256／checkpoint_ordinal／durability／diagnosticを含める。hashは偶発破損検出と対応照合であり、第三者の署名認証を意味しない。

<a id="p-checkpoint-s03"></a>
### P-CHECKPOINT-S03 復元手順と互換性

復元APIは`restore_checkpoint(path, registry, limits)`→新しいPaused RunSessionと新token。①path・manifest・上限、②全file長/hash・過不足、③binary／lock／入力／Registry／codec版、④保存入力からprepare再構築、⑤state内部参照・順序・数値、⑥channelとmodelを非公開arenaへrestore、⑦全成功後にsessionを公開、の順。失敗はCheckpointInvalid／FingerprintMismatch／UnsupportedCodec／RestoreFailedのどれかで返し、runを開始しない。

state検査はFES key一意・phase0/1のDelivery又はphase2のArbitration、未来又は現在以降の時刻、現在時刻のdelta範囲、owner/resource/schema参照、event/token ID<next_event_id、sequence<next_sequence、live tokenと有効予約の一対一、取消済みtombstoneの除去、dirtyと実体所有、metric target・窓・spool offset範囲を確認する。現在candidateは成功境界でFESへ保持されており、未commit batchは保存しない。

復元時にinitialize、finish、過去の送信・観測を再実行しない。新しいsession IDとResumeTokenを発行し、旧processのtoken・ack handleは拒否する。pending制御要求は保存境界で通常の優先順で処理して空にする。保持ackはbundleへ入れず、next_control_idとreplay列で過去IDを再利用しない。stateにはreplay_log全体とcursorを保存し、prefix hashをmanifestの対応先へ含める。旧ackが必要な監査は保存済replay／結果exportを参照する。

初期版は同一binary bytes・同一実装/codec版・同一入力を要求する。異なる版・architecture・endianへ自動変換しない。JSON container自体は独立形式だが、任意codec payloadの移植性を推測しない。network partitionや既存専用engineのcodecは別対応であり、本bundle仕様だけで対応済みにしない。

## アーキテクチャ責務

<a id="p-control-a01"></a><a id="p-control-a02"></a><a id="p-control-a03"></a>
| ID | componentと所有 | 引き渡すもの |
| --- | --- | --- |
| P-CONTROL-A01 | RunSessionがpreparedのArcとowned EngineStateを保有。drive threadだけが変更 | P-CONTROL-S01の状態／境界→D01 |
| P-CONTROL-A02 | Controllerのmutex内にpending／ack／採番／受付watermark | S02のslot・idempotency・replay→D02 |
| P-CONTROL-A03 | FrozenJournalViewとExporter、initialized集合を持つCleanupGuard | S03のfreeze・公開・finish一回→D03 |

<a id="p-metric-a01"></a><a id="p-metric-a02"></a><a id="p-metric-a03"></a>
| ID | componentと所有 | 引き渡すもの |
| --- | --- | --- |
| P-METRIC-A01 | FrozenMetricCatalogV2はRegistry/prepare所有のimmutable値 | S01の合法組合せ／target→D01 |
| P-METRIC-A02 | MetricAccumulatorV2とMetricDeltaはEngine／一callback所有 | S02のchecked演算と窓→D02 |
| P-METRIC-A03 | Schema3Projectorはfrozen viewだけを読む | S03の型・整列・ratio→D03 |

<a id="p-checkpoint-a01"></a><a id="p-checkpoint-a02"></a><a id="p-checkpoint-a03"></a>
| ID | componentと所有 | 引き渡すもの |
| --- | --- | --- |
| P-CHECKPOINT-A01 | FrozenCheckpointCatalogとCheckpointView | S01の全codecと静止点→D01 |
| P-CHECKPOINT-A02 | BundleWriter/Readerがroot descriptorとmanifestを所有 | S02の有界保存と原子的公開→D02 |
| P-CHECKPOINT-A03 | RestoreBuilderが非公開arenaを所有し全件成功後移譲 | S03の照合と副作用禁止→D03 |

## 詳細設計

<a id="p-control-d01"></a>
### P-CONTROL-D01 Owned engineとstep境界

現行`runtime/registered.rs::Engine<'a>`はprepared／registeredを借用しsimulateのstack内で尽きる。これをsessionが所有する`Arc<PreparedSimulation>`と`EngineState`へ分離する。EngineState内にそのArc自身を借用するfieldを作らず、各stepのlocal Arc参照からContextを構築する。unsafeな自己参照やPinを必要としない。

`step()`は一件のcallbackとそのpublishまでを実行し、Committed(key)／NaturalEnd／Failedを返す。queueのcandidateは成功commitまで保持する。dirty封鎖とpreflight失敗もFailedとして成功prefixを維持する。boundary pollは初期化前・初期化transaction直後・成功commit直後にだけ置く。Pausedへの移行後は次候補をpopせず、同時刻の残りもFESに残す。

NetworkStateは`execute_network`のlocal loopからowned backend variantへ移す。分野Deltaのpreflight/publishをstep内に残し、control側がbridgeやTSN状態を直接触らない。legacyの各専用simulateをthread停止で代替するadapterは作らず、未対応capabilityとして拒否する。

<a id="p-control-d02"></a>
### P-CONTROL-D02 要求取込とtoken消費

mutexを一回取得し、その時点のpendingを最大64件移して閉じたsnapshotを作る。slotは結果ackへ移るので総数は増えない。snapshot内でStopを探し、同種FIFOを保ち、S01の優先順で状態を計算してackを確定する。engine実行中にmutexを保持しない。submit／ackのpoisonはControlUnavailableとして返し、未公開simulation効果を成功へ変えない。

tokenはPaused生成時にgenerationをchecked増分して発行する。generation上限ならPauseはResourceExhausted ackを返し、状態・FESを維持する。resumeはsession・fingerprint・generation・状態を検査してから一回だけ消費する。exportや不正resumeでgenerationを進めない。同境界のStopが先ならresumeはSupersededでtokenを無効化する。replayはstable boundary tupleを保存し、通常FESのsequenceやmetricの母数へ加えない。

<a id="p-control-d03"></a>
### P-CONTROL-D03 Freezeと終了処理

freezeはjournal/spoolの確定長・時刻・indexとmetric viewを読取所有値へ固定する。Pausedのlive積分器をHまで破壊的更新せず、window出力はコピーしたstateで計算する。export失敗の後も次候補、ID、token、計測stateのfingerprintが同じであることを試験する。

cleanup guardはinitialized_model_idsの逆順、続いてfactory成功を記録したconstructed_channel_idsの逆順を一回走査し、処理済みflagを先に立ててcallback panicも二重finishへつなげない。自然終端・失敗・closeの全経路で同じguardを使う。freeze済みjournalへの可変参照をfinishへ渡さない。finishの二次診断と一次runtime診断を分け、先にexport済みファイルへ後書きしない。

<a id="p-metric-d01"></a>
### P-METRIC-D01 登録とprepare

`registry/mod.rs`にLegacy/V2 descriptorの内部enumと別登録methodを追加する。prepareでprofileのmetric allowlist、result schema3、target所有、gauge初期値、上限を照合し、canonical順(name,target)のcatalogを作る。既存V1のpublic structへ必須fieldを足さない。V2観測methodはsamplingとsubjectを照合してtyped effectを追加する。

<a id="p-metric-d02"></a>
### P-METRIC-D02 Preflightと時間積分

preflightは対象Accumulatorの小さいcopyとbatchのeffect順から次stateを求める。count/sum/areaのchecked add/mul、観測上限、時刻逆行を一件でも失敗したらbatch全体を棄却する。publishは完成済MetricDeltaとrecordを同時適用する。

窓は時刻の進みに応じて区間を分割し、各窓の(area,duration,count,sum,min,max)を保持する。現在成功時刻tのpoint集計とgauge変更件数は別のtimestamp_bucketに保留する。bucketはtargetごとのcount/sum/min/maxと変更件数を持つ固定幅stateで、標本配列を無限保持しない。全体summaryとjournalは成功時に更新するが、窓へはまだ入れない。tより後の時刻の成功commitをpublishするtransactionでだけ旧bucketを窓へflushする。後のcallbackが失敗したらflushも未公開のままである。正常終端ではt<Tのbucketをflushし、Pause／Stop／失敗prefixのexportではt=Hのbucketを窓から除外して保持する。resume時に同時刻の標本を同bucketへ追加でき、後の成功時刻で一回だけflushする。gauge積分の幅は[前時刻,H)なのでH点変更は面積0。bucketと窓cursorはcheckpointのmetric_accumulatorsへ必ず含める。空時間をFES eventで埋めない。checkpointへ保存するlast_time/value/areaと窓cursorは、非破壊exportで変化しない。

<a id="p-metric-d03"></a>
### P-METRIC-D03 射影と旧版隔離

`output/aggregate.rs`のchecked整数とratioを再利用し、出力validator／CSV／Viewerへschema3を同時追加する。writer側でdescriptorの意味を推測せず、凍結catalogから選択aggregationだけを出す。point/identityの旧APIは従来のrecords／空summaryを保つ。未観測targetはcatalogから補うが、未到達の通信receiverを補う処理とは共有しない。

<a id="p-checkpoint-d01"></a>
### P-CHECKPOINT-D01 Codec取得と一貫view

保存APIはowner専用`checkpoint(destination, limits)`→request handleと確定ack（checkpoint_receiptを含む）とする。Checkpointは共通request commandの一種だが、新規受付はこのowner APIからだけ許す。destinationは明示output rootからの解決済みpath、limitsはS02の型・既定値で操作開始前に検査する。mutex内で共通64枠と次request_idを予約し、同じcritical sectionでcheckpoint_gateを閉じ、先行pending snapshotを取得する。Busy／引数不正はID未消費。受付後の状態不適合・I/O失敗はIDを消費して確定ackを保持する。先行snapshotを通常優先順で処理してからCheckpointのowner_operation境界を一回記録するので、既存要求を追い越さない。再試行は共通`retry(id, Checkpoint{destination,limits})`、保持ack／RequestConflict／forget／DuplicateExpiredも共通規則で、確定receiptを再生成しない。公開前失敗の後に本当に再保存するには新requestを使う。max_exportsは結果exportだけを数え、checkpoint ordinalは独立checked u64（MAXは枯渇sentinel）である。

Registryのcheckpoint対応表はimplementation keyごとにcodec名・版・restore factory・size上限を凍結する。Model／Channelの任意保存hookは対応表と組でのみ利用する。checkpoint開始ではownerがmutex下でcheckpoint_gateを閉じ、既存pending snapshotを取得する。同時点以後のsubmitはBusyで受け付けない。取得済み要求を通常の優先順で処理し、Stop／Resume等でPaused以外になったらCheckpointInvalidStateとしてgateを解除し保存しない。Pausedを保てた場合だけExportingを公開して全可変stateを読む。完了・失敗後に元のPausedへ戻しgateを解除する。受付と保存開始の間に未検査pendingを残さない。

そのPaused ownerがすべての可変stateを一貫した順に読み、各codec出力をsize検査してblob化する。保存hookは&selfで、ContextやFES効果を渡さない。外部副作用を持つcodecは初期capabilityへ登録しない。

<a id="p-checkpoint-d02"></a>
### P-CHECKPOINT-D02 安全な保存と読込

既存`rustix`のdirectory descriptor基準のfile操作を使い、path検査後もnofollowで開く。相対pathを一度検査してから通常path openへ戻さない。content hashが同じblobは一件だけ保存し、manifestの参照数とファイル総数を別に検査する。実読込bytesにも累積上限を適用し、宣言bytesだけを信頼しない。

公開前のstagingを完全検証し、manifest-lastと新規destinationへのrenameを公開点、その後の親directory syncを耐久性確定点とする。rename後のsync失敗はPublishedDurabilityUnknownとしてdestination／manifest_sha256／checkpoint_ordinal／診断を含むreceiptを返す。公開済みbundleを削除せず、ordinalは消費済みとする。同requestの再試行は同receiptを返し、別requestでの同destinationはDestinationExists。rename前の失敗だけが未公開・ordinal未消費である。読込はfiles集合∪{manifest.json}の外のfileも拒否し、余分なblobを無視して別run混在を隠さない。codec payloadに実行コードを入れるplugin loaderは設けない。

<a id="p-checkpoint-d03"></a>
### P-CHECKPOINT-D03 RestoreBuilderと失敗解放

保存inputのbytesとlogical path mappingを持つFrozenInputSourceを使い、既存`prepare_with_registry_and_source(config,cwd,source,registry)`で凍結Registryとcatalogを再構築する。元の絶対pathはsource内の論理名としてのみ扱い、元processのdiskへfallbackしない。INIの相対参照・NED root列挙を含め元の捕捉source集合の読みを再現し、未捕捉pathはMissingCapturedInputで拒否する。initialized保存ならchannel→modelの安定順でrestore factoryを呼ぶがinitializeを呼ばない。uninitializedならModel／Channelのfactoryとrestore factoryを一切呼ばず、初期値だけを持つPaused sessionを構築する。initializedなら復元失敗時に構築済みprefixをdropし、Model／Channel::finishは呼ばない（復元公開前であるため）。restore factoryの私有資源は通常Dropで回収する。全件成功時に保存initialized_model_ids／constructed_channel_ids・journal/FES/metricを取り付け、sessionをPausedとして返す。

cooperative tokenは新processのsession ID/generationへ張り直す。一方、FESにあるTimerTokenのowner/idは予約の同一性として維持し、next_event_idより小さいことを検査する。modelが保存したTimerTokenとFES側の対応をcodec validatorで確認する。checkpoint ordinalはrenameによる公開時に進め（DurabilityUnknownも含む）、結果のorigin IDとevent sequenceを付け替えない。

## 独立検証条件

<a id="p-control-t01"></a>
### P-CONTROL-T01 状態と競合

同時刻のイベント二件の第一commit後にPauseし、第二がpendingのまま、resumeで一回だけ実行されること。ReadyPauseはfactory未呼出、H=0。失敗とPauseの競合はFailed、Stop＋PauseはStopped、自然終了直前のPauseはPaused。全状態×全操作の表を製品で検証する。

<a id="p-control-t02"></a>
### P-CONTROL-T02 Slot・再送・再現

64 slot満杯で新IDを消費せずBusy、同payload retryは同ack、異payloadはRequestConflict、forget後はDuplicateExpired、u64上限後はResourceExhausted。commit ordinalを同じにして制御を再注入すると同じ成功列・ackになること。

<a id="p-control-t03"></a>
### P-CONTROL-T03 Exportとcleanup

Pauseで二回exportしても積分器・FES・tokenが変わらず、出力失敗後のresume結果が非export実行と一致すること。finishはPause／Stop時0、closeで初期化成功実体ごと1、close再実行で追加0。close中finish失敗でも全解放を続けること。Channel factory成功後のModel initialize失敗でも、そのChannelがModel cleanup後に一回finishされること。

<a id="p-metric-t01"></a>
### P-METRIC-T01 Catalog拒否

全合法sampling×aggregationと不正組合せ、gauge初期値欠落、target重複、同名V1/V2、未知target、subject違い、schema2でのV2選択を検証する。prepare失敗はモデルcallbackへ届かない。

<a id="p-metric-t02"></a>
### P-METRIC-T02 独立算術

point 1,2,7はcount3／sum10／min1／max7／mean10/3。gauge初期2、3psで4、7psで9→1、H=10なら面積25、平均2.5。幅4の窓は面積10,13,2。H=7 prefixは面積22、平均22/7、H点の変更は窓へ幅を与えない。point値5@3ps・1@7psでH=7の窓minは5、全体minは1。7psの失敗callbackは新bucketを公開しない。u128上限＋1はbatch失敗で前stateを保持する。

<a id="p-metric-t03"></a>
### P-METRIC-T03 出力と再開

未観測target、空集合null/0、H=0、identityのsummaryなし、binary64 ties-even、CSV文字列表記を比較する。Pauseを7psに入れてexport→resumeした最終面積は25で、途中exportによる二重積分がないこと。

<a id="p-checkpoint-t01"></a>
### P-CHECKPOINT-T01 能力と全状態

全Model／Channel対応時だけ保存でき、片側codec欠落・stateful channelの空扱い・未対応networkで拒否する。ReadyPauseはempty arenaを保存し、復元後一回initializeする。初期化済み保存では復元時initialize／finishが0であること。ReadyPause復元では全factory／restore factoryも0。保存開始前のpending Stop／Resumeは通常処理され、Pausedでなくなれば保存拒否、その後の新規submitはgate中Busyであること。

<a id="p-checkpoint-t02"></a>
### P-CHECKPOINT-T02 破損と公開

file一件欠落、余分file、hash／長さ不一致、同名別run blob、symlink、../、重複key、宣言上限超過、途中書込・rename失敗・rename後parent sync失敗を個別注入する。最後のケースは公開済みDurabilityUnknown receiptとordinal消費を確認する。読込／公開失敗で元bundleとlive sessionのbytes・fingerprintを変えない。

<a id="p-checkpoint-t03"></a>
### P-CHECKPOINT-T03 連続実行との一致

同じcommit ordinalの複数paused_poll／owner_operation groupingも保持する。同時刻残件とcancel済み予約、乱数を持つモデル、stateful channel、複数metric窓、spool prefixを含むfixtureで、連続実行と保存→別process復元→継続の仮想結果を比較する。許容差分はsession ID・保存時刻・export/checkpoint ordinal等の出典だけ。event/token/record順・payload・最終状態・計測は完全一致。

検証入力と期待値は[control-metric-checkpoint-v1.json](../fixtures/control-metric-checkpoint-v1.json)。これは将来製品用の設計vectorで、checkerの合格を製品試験合格としない。

## 実装箇所と順序

| 順序 | 候補箇所 | 完了条件 |
| --- | --- | --- |
| 1 | registry/mod.rs、registry/prepare.rs、types | additive能力表・V2 catalog・codec登録と拒否条件 |
| 2 | runtime/registered.rs、runtime/network.rs | owned state、step、成功境界、既存同期runの結果不変 |
| 3 | 新run/session.rs、control mailbox | 全状態×操作、token、ack、replay、cleanupの試験 |
| 4 | 新runtime/metric_v2.rs、snapshot、output/aggregate.rs | 独立整数・窓・freeze vector一致 |
| 5 | output/validation.rs、Viewer、CLI/API docs | schema3を明示読込・旧版拒否、paused/stopped表示 |
| 6 | 新checkpoint/{catalog,writer,reader,restore}.rs | 完全性・上限・保存失敗・別process継続の試験 |

新module名は配置候補だが、入出力と責務の分割は本書で固定する。実装時に同じ状態を別ownerへ複写して二重管理しない。

```proposal-trace
{"nodes":[
 {"id":"P-CONTROL-R01","stage":"requirement","upstream":[],"anchor":"p-control-r01"},
 {"id":"P-CONTROL-F01","stage":"function","upstream":["P-CONTROL-R01"],"anchor":"p-control-f01"},
 {"id":"P-CONTROL-S01","stage":"spec","upstream":["P-CONTROL-F01"],"anchor":"p-control-s01"},
 {"id":"P-CONTROL-A01","stage":"architecture","upstream":["P-CONTROL-S01"],"anchor":"p-control-a01"},
 {"id":"P-CONTROL-D01","stage":"design","upstream":["P-CONTROL-A01"],"anchor":"p-control-d01"},
 {"id":"P-CONTROL-T01","stage":"verification","upstream":["P-CONTROL-D01"],"anchor":"p-control-t01"},
 {"id":"P-CONTROL-R02","stage":"requirement","upstream":[],"anchor":"p-control-r02"},
 {"id":"P-CONTROL-F02","stage":"function","upstream":["P-CONTROL-R02"],"anchor":"p-control-f02"},
 {"id":"P-CONTROL-S02","stage":"spec","upstream":["P-CONTROL-F02"],"anchor":"p-control-s02"},
 {"id":"P-CONTROL-A02","stage":"architecture","upstream":["P-CONTROL-S02"],"anchor":"p-control-a02"},
 {"id":"P-CONTROL-D02","stage":"design","upstream":["P-CONTROL-A02"],"anchor":"p-control-d02"},
 {"id":"P-CONTROL-T02","stage":"verification","upstream":["P-CONTROL-D02"],"anchor":"p-control-t02"},
 {"id":"P-CONTROL-R03","stage":"requirement","upstream":[],"anchor":"p-control-r03"},
 {"id":"P-CONTROL-F03","stage":"function","upstream":["P-CONTROL-R03"],"anchor":"p-control-f03"},
 {"id":"P-CONTROL-S03","stage":"spec","upstream":["P-CONTROL-F03"],"anchor":"p-control-s03"},
 {"id":"P-CONTROL-A03","stage":"architecture","upstream":["P-CONTROL-S03"],"anchor":"p-control-a03"},
 {"id":"P-CONTROL-D03","stage":"design","upstream":["P-CONTROL-A03"],"anchor":"p-control-d03"},
 {"id":"P-CONTROL-T03","stage":"verification","upstream":["P-CONTROL-D03"],"anchor":"p-control-t03"},
 {"id":"P-METRIC-R01","stage":"requirement","upstream":[],"anchor":"p-metric-r01"},
 {"id":"P-METRIC-F01","stage":"function","upstream":["P-METRIC-R01"],"anchor":"p-metric-f01"},
 {"id":"P-METRIC-S01","stage":"spec","upstream":["P-METRIC-F01"],"anchor":"p-metric-s01"},
 {"id":"P-METRIC-A01","stage":"architecture","upstream":["P-METRIC-S01"],"anchor":"p-metric-a01"},
 {"id":"P-METRIC-D01","stage":"design","upstream":["P-METRIC-A01"],"anchor":"p-metric-d01"},
 {"id":"P-METRIC-T01","stage":"verification","upstream":["P-METRIC-D01"],"anchor":"p-metric-t01"},
 {"id":"P-METRIC-R02","stage":"requirement","upstream":[],"anchor":"p-metric-r02"},
 {"id":"P-METRIC-F02","stage":"function","upstream":["P-METRIC-R02"],"anchor":"p-metric-f02"},
 {"id":"P-METRIC-S02","stage":"spec","upstream":["P-METRIC-F02"],"anchor":"p-metric-s02"},
 {"id":"P-METRIC-A02","stage":"architecture","upstream":["P-METRIC-S02"],"anchor":"p-metric-a02"},
 {"id":"P-METRIC-D02","stage":"design","upstream":["P-METRIC-A02"],"anchor":"p-metric-d02"},
 {"id":"P-METRIC-T02","stage":"verification","upstream":["P-METRIC-D02"],"anchor":"p-metric-t02"},
 {"id":"P-METRIC-R03","stage":"requirement","upstream":[],"anchor":"p-metric-r03"},
 {"id":"P-METRIC-F03","stage":"function","upstream":["P-METRIC-R03"],"anchor":"p-metric-f03"},
 {"id":"P-METRIC-S03","stage":"spec","upstream":["P-METRIC-F03"],"anchor":"p-metric-s03"},
 {"id":"P-METRIC-A03","stage":"architecture","upstream":["P-METRIC-S03"],"anchor":"p-metric-a03"},
 {"id":"P-METRIC-D03","stage":"design","upstream":["P-METRIC-A03"],"anchor":"p-metric-d03"},
 {"id":"P-METRIC-T03","stage":"verification","upstream":["P-METRIC-D03"],"anchor":"p-metric-t03"},
 {"id":"P-CHECKPOINT-R01","stage":"requirement","upstream":[],"anchor":"p-checkpoint-r01"},
 {"id":"P-CHECKPOINT-F01","stage":"function","upstream":["P-CHECKPOINT-R01"],"anchor":"p-checkpoint-f01"},
 {"id":"P-CHECKPOINT-S01","stage":"spec","upstream":["P-CHECKPOINT-F01"],"anchor":"p-checkpoint-s01"},
 {"id":"P-CHECKPOINT-A01","stage":"architecture","upstream":["P-CHECKPOINT-S01"],"anchor":"p-checkpoint-a01"},
 {"id":"P-CHECKPOINT-D01","stage":"design","upstream":["P-CHECKPOINT-A01"],"anchor":"p-checkpoint-d01"},
 {"id":"P-CHECKPOINT-T01","stage":"verification","upstream":["P-CHECKPOINT-D01"],"anchor":"p-checkpoint-t01"},
 {"id":"P-CHECKPOINT-R02","stage":"requirement","upstream":[],"anchor":"p-checkpoint-r02"},
 {"id":"P-CHECKPOINT-F02","stage":"function","upstream":["P-CHECKPOINT-R02"],"anchor":"p-checkpoint-f02"},
 {"id":"P-CHECKPOINT-S02","stage":"spec","upstream":["P-CHECKPOINT-F02"],"anchor":"p-checkpoint-s02"},
 {"id":"P-CHECKPOINT-A02","stage":"architecture","upstream":["P-CHECKPOINT-S02"],"anchor":"p-checkpoint-a02"},
 {"id":"P-CHECKPOINT-D02","stage":"design","upstream":["P-CHECKPOINT-A02"],"anchor":"p-checkpoint-d02"},
 {"id":"P-CHECKPOINT-T02","stage":"verification","upstream":["P-CHECKPOINT-D02"],"anchor":"p-checkpoint-t02"},
 {"id":"P-CHECKPOINT-R03","stage":"requirement","upstream":[],"anchor":"p-checkpoint-r03"},
 {"id":"P-CHECKPOINT-F03","stage":"function","upstream":["P-CHECKPOINT-R03"],"anchor":"p-checkpoint-f03"},
 {"id":"P-CHECKPOINT-S03","stage":"spec","upstream":["P-CHECKPOINT-F03"],"anchor":"p-checkpoint-s03"},
 {"id":"P-CHECKPOINT-A03","stage":"architecture","upstream":["P-CHECKPOINT-S03"],"anchor":"p-checkpoint-a03"},
 {"id":"P-CHECKPOINT-D03","stage":"design","upstream":["P-CHECKPOINT-A03"],"anchor":"p-checkpoint-d03"},
 {"id":"P-CHECKPOINT-T03","stage":"verification","upstream":["P-CHECKPOINT-D03"],"anchor":"p-checkpoint-t03"}
]}
```

2026-10-17採用作業記録：本書のP-IDを[正式採用仕様](../../specs/後続拡張採用詳細機能仕様書.md)へ接続した。[現行API・scope](../../guide/reference/後続10分野の現行実装と利用境界.md)と[製品検証記録](../../verification/results/extension-product-2026-10-17.md)を参照。設計vectorと過去のdesign-only報告は履歴のまま保持し、製品合格とは区別する。文書版・履歴は本pushの改訂として確定した。
