# CANFD・100BASE-T1詳細機能仕様書

文書バージョン：`0.1.0`
対象GitHubバージョン：`未リリース（main @ 7738b55）`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版草案。CAN FD・100BASE-T1の実装契約と独立解析fixture |

文書ID：`spec-original-network`

状態：仕様・設計の決定事項を記述する草案。製品シミュレータは未実装であり、解析fixtureの合格を製品試験合格とみなさない。

既存[Classical CAN](CANモデル詳細機能仕様書.md)と[Ethernet v1](Ethernetモデル詳細機能仕様書.md)/[v2媒体](Ethernet媒体拡張詳細機能仕様書.md)を保持し、以下を別profileとして明示選択する。追加profileも共通の時刻ps、phase 0完了→phase 1生成→phase 2仲裁、commit journal、[0,T)停止規則を使用する。

<a id="canfd"></a>

## 1. CAN FD専用バス

```trace
{
  "id": "spec-original-network#canfd",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0174",
    "DIR-REQ-0175",
    "DIR-REQ-0176",
    "DIR-REQ-0177",
    "DIR-REQ-0178",
    "DIR-REQ-0179",
    "DIR-REQ-0180",
    "DIR-REQ-0181"
  ],
  "upstream": [
    "DIR-FUNC-0042"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 選択と構成 | INI `model-profile = "can.fd.precomputed.v1"`、`model-config = "fd.model.json"`。model-config全キー必須の厳密rootは`{schema_version:1,profile:"can.fd.precomputed.v1"}`のみ。モデル値はNED/INIを正本とし、このJSONはprofile版の照合用。相対パスはINI親。登録型`dir.canfd.ControllerV1/BusV1`と`dir.link.FixedDelay`。Controllerのtx/rxとBusのtx_suffix/rx_suffixをCC v1と同じpeer対で接続。Bus1個・Controller2個以上、全端常時active、ideal ACK成立。ControllerのqueueCapacity/txProcessingDelay/rxProcessingDelay/rxFilterとchannel delayはCC v1の型・範囲を使用する |
| Bus | 必須`nominalBitrate`=1..1000000整数bps、`dataBitrate`=nominalBitrate..8000000整数bps。両値は本抽象profileの受理範囲。全端が同じBus値を参照する。注：実機対応最大速度を保証する値ではない |
| workload | 共通schema_version=2、generators配列。各generator全キー必須`{id,kind,node,times_ps,frame}`。kind=`can.fd.explicit.v1`、idは共通識別子、nodeはController完全パス、times_psは非負u64正規十進文字列のps配列、非減少・重複可。生成順は(時刻,id UTF-8辞書順,配列ordinal)。周期負荷は時刻配列へ事前展開して使用する |
| frame | 全キー必須`{format,id,data,brs,wire}`。format=standard/extended、id整数範囲=11/29bit、data偶数hex。byte長は0..8,12,16,20,24,32,48,64だけ、DLCは0..15対応表から導出。brsはJSON bool。全階層未知/欠落/重複キーを準備失敗とする。注：暗黙padding、remote frame、CCとの同一バス混在、エラー注入、ESI/error-passiveは本profileの対象外。ESIはerror-active固定 |
| wire | 全キー必須`{nominal_bits,data_bits,evidence,binding_sha256}`。bit数はJSON整数0..1000000でnominal_bits>=1、data_bits>=8*payload_bytes。evidenceは1..512文字の非空文字列で、外部計算器の版・設定又は測定ファイル識別子を記載する。brs=falseはdata_bits=0かつnominal_bits>=8*payload_bytesを要求し、上記data_bits下限はbrs=trueに適用する。brs=trueはdata_bits>=1も要求する |
| 位相の定義 | nominal_bits/data_bitsはSOFからEOFまでの公称/データ速度で消費する等価bit数（CRC・動的/固定stuff・ACK・EOFを含む）を外部で算定した整数入力。intermission3公称bitは含めない。位相遷移sample pointによる分数bit時間は外部で整数位相長へ丸めたモデル値を使う。証跡にこの丸め規則を記す。注：本版はbitstream生成、CRC/bit数の規格適合を検証しない。証跡文字列の存在は内容の正しさを保証しない |
| 内容結合 | binding_sha256は小文字hex64桁。ASCII `format + "\|" + 十進id + "\|" + lowercase(data) + "\|" + (brsなら"1"他は"0") + "\|" + 十進N + "\|" + 十進D + "\|" + 十進Rn + "\|" + 十進Rd` のSHA-256と一致させる（全整数は先頭0なし）。分離文字はASCII pipe1文字。DLCはdata長から一意に導出する。証跡の変更時も当該内容と計算条件の独立検証を要する |
| 誤用防止 | 結果のfidelity=`externally-precomputed-phase-bits`、wire_validation=`structural-only`を必須とする。証跡と入力frame/速度/bit数を結果へそのまま保存する。計測・信頼できる外部計算からbit数を供給し、証跡を独立に検証する責務は利用者にある。内容依存CRC/stuffing算定と完全なISO bitstream適合は将来の別codecで扱う。解析fixture値はsyntheticであり合法なFD波形を表すとは主張しない |
| 時間 | N=nominal_bits、D=data_bits、Rn/Rd=速度。duration=ceil(10^12*(N*Rd+D*Rn)/(Rn*Rd))、EOF=SOF+duration、release=SOF+ceil(10^12*((N+3)*Rd+D*Rn)/(Rn*Rd))。一つの和に対して一度ceilする。BRS=falseはD=0。EOFで送信成功、releaseで次仲裁を許可。受信到達=EOF+送信元Controller→Bus channel delay+各Bus→受信Controller channel delay、完了=到達+rxProcessingDelay |
| 仲裁 | CC v1の非stuff仲裁列比較（standard ID11,0,0 / extended baseID11,1,1,extID18,0）と同一format/idの送信者一意検証を再利用する。FDのRRS固定0で同じ大小関係を得る。各Controllerはready queue最優先を提出、敗者保持、SOF後の後着は次仲裁。SOF自体に追加仲裁時間を加えない |
| queue | generated+txProcessingDelayでready。容量はready待機件数、送信中を除く。ready時満杯はdrop-newest、phase順と生成順で同時readyを処理。次SOFで待機から除去。全Controllerへ送信元を除く同報を行い、敗者も受信する。rxFilterの構文と観測時判定はCC v1を使用し、不適合はfiltered終端、適合だけrx処理へ進む。CAN v1同様にrequest_id/generated/ready/sof/eof/releaseと受信行を追跡する |
| 停止・異常 | Tと同時の完了/生成/解放/受信は未実行、部分送信はtransmitting、EOF<Tかつrelease>=Tはserialized。受信到達<T・処理完了>=Tはprocessing。未知profile/入力/所有者違反はE-0001、内部不変条件はE-0002、時刻overflowはE-0004。予約時刻もchecked u128計算後u64検査、失敗batchは非公開、直前commit prefixを出力する |
| 結果 | 共通result schema2のmodel_recordsでfd_frame（frame_id、format/id/data/DLC/brs、wire、速度、fidelity）、fd_request（request_id、frame_id、node、generated/ready/sof/eof/release、state、drop_reason）、fd_reception（request_id、receiver、arrival/completed、state）を記録する。各行idは実行内一意、未到達時刻はnull。計画時刻とcommit済時刻は分け、未完了を成功計数しない。状態集合はpending/queued/transmitting/serialized/dropped及びpending/processing/completed/filtered |

| 出力型 | 正本契約 |
| --- | --- |
| 共通型と外枠 | S=string、D=非負u64正規十進文字列、J=JSON整数、B=bool、?=null可。全dataキー必須、未知/重複拒否。ModelRecord共通外枠は拡張共通仕様どおり、schema_version=1（JSON整数）、origin_request_id=null、time_ps=最後のcommit時刻（静的行0）、request_idは要求ID又はnull。schema_nameは以下の3名に固定。record_idはframeにはgenerator ID、requestにはgenerator ID+":"+十進ordinal、receptionにはrequest ID+":"+receiver完全パス。generator IDはASCII `[A-Za-z_][A-Za-z0-9_]*`としdelimiter衝突を避ける |
| fd_frame | schema_name=`dir.canfd.frame`、subject=送信Controller、request_id=null。data=`{format:S,id:J,data:S,dlc:J,brs:B,nominal_bits:D,data_bits:D,evidence:S,binding_sha256:S,nominal_rate:D,data_rate:D,fidelity:"externally-precomputed-phase-bits",wire_validation:"structural-only"}`。dataはlowercase hex。全prepare成功後の静的行 |
| fd_request | schema_name=`dir.canfd.request`、subject=送信Controller。data=`{frame_id:S,source:S,bus:S,generated_ps:D,ready_ps:D?,sof_ps:D?,eof_ps:D?,release_ps:D?,planned_ready_ps:D,planned_eof_ps:D?,planned_release_ps:D?,state:S,drop_reason:S?}`。stateはpending/queued/transmitting/serialized/dropped、drop_reasonはqueue_full又はnull。実績欄は各callback commit時だけ設定 |
| fd_reception | schema_name=`dir.canfd.reception`、subject=受信Controller。EOF commit時に作成。data=`{frame_id:S,receiver:S,planned_arrival_ps:D,planned_completed_ps:D,arrival_ps:D?,completed_ps:D?,state:S}`。state=pending/processing/completed/filtered。filter拒否はarrivalのみ設定、completed=null。planned_completedはfilter適合時の仮定予定時刻 |
| 通知 | tx=`dir.canfd.TxRequestV1`、rx=`dir.canfd.NotificationV1`、schema版1。compact UTF-8 JSON、キー辞書順、全キー必須。共通body=`{kind:S,request_id:S,source:S,bus:S,receiver:S?,frame_id:S,time_ps:D,generation:D}`。kind=ready/arrival、readyはreceiver=null/time_ps=ready、arrivalはreceiver解決済/time_ps=planned_arrival。FrameCacheを参照しEnvelope時刻・送受信handle所有・不変generationを照合。不正はE-0002。txは制御通知でchannel遅延を足さず、rxだけ送信元tx+受信rx遅延を合成する |

| metric ID | version / unit / value_kind / sampling / aggregation | 対象・定義 |
| --- | --- | --- |
| `canfd.generated` | 1 / count / integer / summary / sum | Controller及び$all、generated commit件数 |
| `canfd.serialized` | 1 / count / integer / summary / sum | Controller及び$all、EOF commit件数 |
| `canfd.dropped` | 1 / count / integer / summary / sum | Controller及び$all、queue_full commit件数 |
| `canfd.received` | 1 / count / integer / summary / sum | 受信Controller及び$all、completed commit件数 |
| 集計共通 | 上記4指標だけを登録。Hまでの成功commit prefixから計算し0件も出力。start=0/end=H、request_id/receiver/reason/sample_count=null。空窓の別行やCC37指標の暗黙追加は対象外（注） | generated=queued+pending+transmitting+serialized+dropped、受信件数は送信件数と分離 |

<a id="t1"></a>

## 2. 100BASE-T1全二重

```trace
{
  "id": "spec-original-network#t1",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0182",
    "DIR-REQ-0183",
    "DIR-REQ-0184",
    "DIR-REQ-0185"
  ],
  "upstream": [
    "DIR-FUNC-0043"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 契約 |
| --- | --- |
| 選択 | INI `model-profile = "ethernet.l2.100base-t1.v1"`と`model-config`。既存v2のEndpointV2/SwitchV2/LinkV2、model-config schema2、workload schema2のethernet.explicit.v1、Frame/FCS、tree、FDB、FIFO、処理遅延を再利用する |
| 専用profile検証 | 全physical_linksのphy_mode=`100base-t1`、duplex=`full`、両方向Link bitrate=100000000bpsを必須とする。a_phy/b_phyはmaster/slave一つずつ、tx_latency_ps/rx_latency_psは明示非負u64十進文字列。peer/role/重複/未知キー検証はv2と同じ。注：v2へ100base-t1入力を追加する変更ではない |
| link-up | 時刻0からup、roleは固定clock属性。注：training/auto negotiation、波形、符号化、EMC、規格全体適合、link faultは対象外。PHY latency0は理想値として記録する |
| MAC | R=100000000、M=FCSを含むMAC frame byte数（v1 frame構築で64..1518）。wire_bits=8*(M+8)、occupied_bits=8*(M+20)。EOF=SOF+ceil(wire_bits*10^12/R)、release=SOF+ceil(occupied_bits*10^12/R)。送信方向は独立し同時SOFを許す |
| PHY pipeline | i→jのmdi_sof=SOF+TX_i、mdi_eof=EOF+TX_i、peer_mdi_sof=mdi_sof+P、peer_mdi_eof=mdi_eof+P、arrival=EOF+TX_i+P+RX_j。受信処理遅延はarrival後に一回加える。MAC releaseへPHY遅延を加えない。符号化後symbol rateでMAC時間を再計算する処理は対象外（注） |
| runtime・結果 | v2のfull/T1 phase、AttemptMap、型付きArrival、原子journal、queueCapacity/drop-newest、switch store-and-forward、[0,T)停止、E-0001/0002/0004を再利用。profileとphy_mode=100base-t1、両端role/PHY値をphy_linkへ保存し、frame/transfer/reception/attemptはv2同型で保存する |

## 3. 根拠・受入と忠実度

Boschの[CAN FD紹介](https://www.bosch-semiconductors.com/products/ip-modules/can-protocols/can-fd/)はpayload最大64byteを説明する。DLC対応長は[Bosch M_TTCAN manual](https://www.bosch-semiconductors.com/media/ip_modules/pdf_2/m_can/mttcan_users_manual_v330.pdf)のData Length Code表を参照した。TIの[100BASE-T1説明](https://www.ti.com/lit/wp/szzy009/szzy009.pdf)は100Mbps・単一pair・全二重を説明する。これらは採用範囲の根拠であり、FD位相長の自動算定や物理層適合を保証するものではない。

[詳細設計](../../design/CANFD・100BASE-T1詳細設計書.md) → [TEST0040–0043](../../verification/cases/CANFD・100BASE-T1検証仕様書.md)。受入は[DIR-AC-0043](../../要件定義書.md#dir-ac-0043)・[DIR-AC-0044](../../要件定義書.md#dir-ac-0044)。
