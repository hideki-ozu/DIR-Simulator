# Ethernet負荷・QoS詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 45ce163`
予定公開版：`v1.1.0`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-04` | 初版：lazy generator、class送信選択と原子的容量判定、親経路によるフロー計測とviewer設計 |

文書ID：`design-ethernet-qos`
文書状態：開発版。製品検証の証拠は検証仕様へ記録する。

<a id="workload"></a>

## 1. profile選択・負荷cursor

```trace
{"id":"design-ethernet-qos#workload","stage":"design","requirements":["DIR-REQ-0218"],"upstream":["architecture#arch-ethernet-qos"],"state":"confirmed","pending":[]}
```

既存PreparedSimulationへoptional PreparedEthernetを追加し、CAN/Gatewayフィールドを維持する。prepareで選択profileとpayloadを一致させ、runtimeはexact profileでCAN又はEthernetエンジンへdispatchする。NEDの構文・展開・接続経路は既存resolverを再利用し、Linkのbitrate/delayをdescriptorとして保持する。

EthernetGeneratorは基本v1のtimes_psと、追加profileのEthernetScheduleを保持する。time(ordinal)はu128の論理時刻又は枯渇を返す。periodicとburstは式から候補を求め、全時刻列を展開しない。各generatorのordinalと次候補だけを持ち、dispatcherが最小時刻の同時候補をID/ordinal順に生成する。T以後だけの負荷は生成0件のまま論理候補を保持する。

周期のcountとburst_countの積・時刻は広い整数で検査し、ordinal更新・イベント連番・delta・実行時刻の算術失敗はE-0004で失敗候補を未commitにする。0delay処理は同phase1、正delay完了はphase0からphase1へ通知する。過去phaseへの同時刻予約だけdeltaを進める。

flow設定はgeneratorから正規化する。同flowのdst_mac/priority/deadline不一致はprepare失敗。全frame bytesを準備でcacheし、生成時に元frameのフロー属性を一度記録する。元frame IDはgenerator ID:ordinalで維持する。

<a id="queues"></a>

## 2. classキュー・送信器・atomic fanout

```trace
{"id":"design-ethernet-qos#queues","stage":"design","requirements":["DIR-REQ-0219","DIR-REQ-0220"],"upstream":["architecture#arch-ethernet-qos"],"state":"confirmed","pending":[]}
```

PreparedEthernet.outputsはoutputパス順、各queuesはpriority順。EthernetOutputConfigはschedulerと8個のEthernetQueueConfigを保持する。送信器のactive transferは方向別に一つ、待機はclass別VecDeque、待機MAC byte数はclass別checked整数とする。基本v1は単一FIFOで同じ時刻・状態規則を維持する。

offerの前に全候補のID一意性、所属方向、class、待機数とbyte数、dirty登録・予約の連番容量を検査する。容量不足は通常dropとして全候補の結果を作り、別egressの成功を妨げない。算術・予約不変条件失敗では候補一括を未公開にする。モデル全体のcloneやcallback再実行を通常経路へ置かない。

transfer作成位置を安定offer序として保持する。FIFOは各class先頭の最小offer序、strict_priorityは最大priorityの先頭。SOF予約はEOF/release/arrivalのchecked時刻を確認してから、待機削除・byte減算・active設定・予定時刻・queue観測を確定する。EOFとreleaseはSOF原点から別々にceil算出する。

class/portの観測点は同じcallbackに属し、effect_seqを重複させない。満杯offerでも長さ・byteの不変点を記録する。releaseでactiveを解放しdirtyへ登録する。arrivalはEOF成功後だけ確定し、T境界や失敗時の予定だけの受信行を作らない。

<a id="observations"></a>

## 3. 結果projection・フロー分析・viewer

```trace
{"id":"design-ethernet-qos#observations","stage":"design","requirements":["DIR-REQ-0221","DIR-REQ-0222","DIR-REQ-0223"],"upstream":["architecture#arch-ethernet-qos"],"state":"confirmed","pending":[]}
```

EthernetSnapshotはFrame/Transfer/Receptionをnative整数で保持する。共通Snapshotは終了、確定callback数、未処理数、Point、診断を持つ。outputはEthernet専用projectionへdispatchし、既存CAN/GWのlineage・bus指標・初期stateをEthernetへ適用しない。

schema2包絡とcanonical writer・manifest-last公開・CSV列は既存共通処理を再利用する。profileに応じてframe/transferの版と追加キーを選択し、基本版へ追加キーを混入させない。metadataは初期queue空・方向idle、Link値、Endpoint MAC、FDB、cursor0、class容量・schedulerを正規形へ保存する。

分析はframe_id→Frame、transfer_id→Transfer、incoming transfer_id→ReceptionのBTreeMapを構築する。完了受信だけをdelivery標本とし、parent_transfer_idを逆向きにsourceまでたどる。参照欠落、cycle、未完了親、4成分合計とdelivery不一致は出力不変条件違反とする。平均はu128和/標本数を既存の単一丸めratioでbinary64へ変換する。分位値は整数ソートとnearest rankで選ぶ。

viewerはCANとは独立したEthernet pure replayモデルを持ち、shared loaderでprofileごとにdashboardを選択する。raw schema、record一意性、親子参照、方向、実績時刻の範囲と順序、schema版を検証する。BigIntで仮想psを保持し、描画用の位置比だけNumberへ変換する。snapshot(time)は毎回実績から復元し、巻戻しでmutableな通信状態を残さない。

連続再生はactive方向線の強調、stepは直前イベント区間のtransferを実際のfrom→toへ再演する。quiet deviceはmetadataのtopologyから表示する。フロー表は登録済みsummaryを使用し、未完了・dropと完了受信の期限判定を分ける。

初期cursorはu64時刻`next_time_ps`とu128論理候補`next_candidate_ps`を分ける。範囲外の候補は次時刻null・候補値保持、枯渇は両方nullとし、未実行の将来予定をmetadataでも切詰めない。
