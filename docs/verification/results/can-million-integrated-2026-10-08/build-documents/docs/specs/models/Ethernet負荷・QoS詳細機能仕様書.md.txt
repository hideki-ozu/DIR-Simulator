# Ethernet負荷・QoS詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`main @ 45ce163`
予定公開版：`v1.1.0`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-04` | 初版：周期・バースト、8class優先度・MAC byte容量、フロー遅延・期限・経路内訳とviewer契約 |

文書ID：`spec-ethernet-qos`
文書状態：開発版。実装と製品試験を同じ作業ツリーで整備済み。v1.1.0向けPRの初回pushで文書版・履歴を確定。

## 1. 選択と互換性

INIの`model-profile = "ethernet.l2.qos.v1"`を明示選択する。Endpoint・Switch・LinkのNED型、untagged Ethernet II、CRC32、全二重、connected tree、静的FDB・flood、独立固定処理遅延、整数ps、[0,T)停止は[基本L2仕様](Ethernetモデル詳細機能仕様書.md)を継承する。基本L2 v1の入力・結果・拒否条件を維持する。

本profileは指定した通信優先度を扱う。VLANタグ、PCPのwire符号化、MAC学習、TSN、PAUSE、上位protocol、有限RX処理資源は将来の独立契約で扱う。

<a id="workload"></a>

## 2. 決定的な周期・バースト負荷

```trace
{"id":"spec-ethernet-qos#workload","stage":"spec","requirements":["DIR-REQ-0218"],"upstream":["DIR-FUNC-0052"],"state":"confirmed","pending":[]}
```

workloadのrootは`{schema_version:2,generators:[]}`。各generatorに`id,node,kind,frame,flow_id,priority,deadline_ps`とkind別の下表fieldを必須指定する。`frame`は基本L2の`{dst_mac,ether_type,data}`。`id`は実行内で一意、`flow_id`もASCII識別子。`priority`はJSON整数0～7（bool不可）、`deadline_ps`は生成から受信処理完了までの相対期限D又はnull。Dはu64の正規非負十進文字列。未知・欠落・重複キーを準備時に拒否する。

| kind | 必須の追加field | 発火時刻・境界 |
| --- | --- | --- |
| `ethernet.explicit.v1` | `times_ps:D[]` | 非減少の入力列。空列・同時刻重複を受理 |
| `ethernet.periodic.v1` | `start_ps:D,phase_ps:D,period_ps:D,end_ps:D?,count:D?` | `start+phase+ordinal*period`。period>0、phase<period、count件まで、endがあれば時刻<end |
| `ethernet.burst.v1` | `start_ps:D,period_ps:D,burst_count:D?,frames_per_burst:D,spacing_ps:D,end_ps:D?` | `start+floor(ordinal/n)*period+(ordinal%n)*spacing`。n>0、period>0、`(n-1)*spacing<period`。burst_countはバースト数、endは個々の発火に適用 |

count/burst_count=0を受理し生成0件とする。spacing=0なら同時刻にordinal順で生成する。期限、終了時刻、相対値はnullの意味を保持し、0と混同しない。計算はu128で行い、global T以後の候補を実行しない。準備時にはT以後・空負荷のframeも検証する。generator配列順・JSONキー順を変えても、同時発火はgenerator ID辞書順・ordinal順で一致する。

同一flow_idを複数generatorで使う場合、priority・deadline_ps・dst_macを一致させる。異なる送信元からの同じフローを許す。Flowは計測の分類であり、bandwidth reservationや接続確立を発生させない。

<a id="queues"></a>

## 3. 出力の優先度別キューとbyte容量

```trace
{"id":"spec-ethernet-qos#queues","stage":"spec","requirements":["DIR-REQ-0219","DIR-REQ-0220"],"upstream":["DIR-FUNC-0053"],"state":"confirmed","pending":[]}
```

model-configのrootは`{schema_version:2,endpoints:[],switches:[],outputs:[]}`。Endpoint/Switchの内容は基本L2と同じ。outputsは接続済みoutputを過不足なく一度ずつ列挙する。

```json
{"port":"Main.a.tx","scheduler":"strict_priority","queues":[
  {"priority":0,"capacity_frames":"64","capacity_bytes":null},
  {"priority":1,"capacity_frames":"64","capacity_bytes":null},
  {"priority":2,"capacity_frames":"64","capacity_bytes":null},
  {"priority":3,"capacity_frames":"64","capacity_bytes":null},
  {"priority":4,"capacity_frames":"64","capacity_bytes":null},
  {"priority":5,"capacity_frames":"64","capacity_bytes":null},
  {"priority":6,"capacity_frames":"64","capacity_bytes":null},
  {"priority":7,"capacity_frames":"64","capacity_bytes":null}
]}
```

priority0～7の全8キューを各outputへ一度ずつ設定し、入力配列順から独立に正規化する。capacity_framesはDで0～u32max、capacity_bytesはD又はnull。byte容量にはpadding・FCSを含むMAC bytesを算入し、preamble・IFG・payloadだけの長さは容量に使わない。nullはbyte上限なし。

NEDのqueueCapacityは、そのoutputの全classを合わせた待機フレーム数上限として併用する。入場にはport全体、classフレーム数、class byte数の全条件を満たす必要がある。送信中のcopyは各容量から除外する。0容量、byte超過、満杯では新着copyをqueue_fullとして破棄し、既存copyと他egressへのcopyは保持する。

各class内はoffer成功順FIFO。scheduler=`fifo`は各class先頭のうち最古のofferを選択し、`strict_priority`は非空の最大priorityを選択する。どちらも非プリエンプト。phase0完了・解放→phase1到着・offer→phase2送信選択の順で、phase1は同時刻phase2の取出しを先取りしない。

classのqueue_idは`output.queue.priority`、port合計は`output.queue`。classごとのqueue_length/queue_bytes、最大値、[0,H)時間平均とport合計queue_lengthを記録する。容量不足で長さが変わらないofferも観測できるよう記録する。

<a id="observations"></a>

## 4. フロー計測・遅延内訳・可視化

```trace
{"id":"spec-ethernet-qos#observations","stage":"spec","requirements":["DIR-REQ-0221","DIR-REQ-0222","DIR-REQ-0223"],"upstream":["DIR-FUNC-0054"],"state":"confirmed","pending":[]}
```

外側結果schema2を使用する。`ethernet.frame`/2は基本版のdataへ`flow_id:S,priority:D,deadline_ps:D?`、`ethernet.transfer`/2は`queue_id:S,priority:D`を追加する。`ethernet.reception`/1は基本版を維持する。一実行内で同schema_nameの版を混在させない。元frame、copy、receptionのIDと実績／予定時刻の意味を維持する。

| 対象 | 指標と定義 |
| --- | --- |
| `@flow:<flow_id>` | generated元frame数、received受信判断数、copy_dropped破棄copy数、filtered/processing受信判断数、unfinished_copies未到達copy数。複製による母数の違いを保持 |
| `@flow:<flow_id>:<Endpoint>` | received/filtered/processing、受信遅延、期限。通信していないEndpointにも件数0を出力 |
| 受信遅延 | `ethernet.flow.delivery_mean_ps`（number）、max/p50/p95/p99/jitter（integer又はnull）。受信処理完了−生成を標本とし、分位値はnearest rank=`ceil(n*p/100)`番目、jitter=max−min。0標本はnull、1標本jitter=0 |
| 期限 | `ethernet.flow.deadline_sample_count`は非null期限を持つ完了受信数、deadline_missedは受信遅延>期限、deadline_miss_ratioはその比。期限と等しい受信は達成、未完了は期限判定の標本に含めない |
| 遅延内訳 | 完了受信の親transfer列をsourceまでたどる。queue_wait=Σ(SOF−offer)、serialization=Σ(EOF−SOF)、propagation=Σ(arrival−EOF)、processing=source TX処理+各Switch/Endpoint処理。4成分の和を受信遅延へ照合 |
| 内訳の出力 | 受信ごとの`ethernet.queue_wait_ps/serialization_ps/propagation_ps/processing_ps`点、およびflowの各`*_mean_ps`。request_idとreceiverを保持 |

flow件数・期限件数はcount/integer/summary/sum、期限比は1/number/summary/ratio、平均はps/number/summary/sample_mean、maxはps/integer/summary/max、分位値はps/integer/summary/nearest_rank、jitterはps/integer/summary/range。queue_bytesはB/integer/point/identity、queue_bytes_maxはB/integer/summary/max、queue_bytes_meanはB/number/summary/time_mean。copy_droppedはhopコピーの件数であり、元frameの終端損失率とは別の指標である。

metadataに選択profile、版付きschema集合、全キュー設定、フローのpriority・期限を保存する。viewerはEndpoint/Switch、方向別link、port/class別の待機、フロー・priority・期限、copy経路と実績／予定の違いを表示する。再生中は方向線を強調し、step操作は到達先イベントの直前区間を再演する。同じ到達先への前進／巻戻しは同じ表示にする。

[詳細設計](../../design/Ethernet負荷・QoS詳細設計書.md)と[検証仕様](../../verification/cases/Ethernet負荷・QoS検証仕様書.md)で実装と受入条件を対応付ける。

初期generator stateの`next_time_ps`はu64範囲内の次時刻D?で、枯渇又は範囲外ならnull。QoSの`next_candidate_ps`は論理候補をu128範囲の正規十進文字列で保存し、枯渇だけnullとする。u64範囲外の候補を切詰めて実行しない。
