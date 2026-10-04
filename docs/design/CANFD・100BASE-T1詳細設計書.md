# CANFD・100BASE-T1詳細設計書

文書バージョン：`1.1.2`
対象GitHubバージョン：`main @ 7bb9bfc`
予定公開版：`v1.1.2`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-04` | 外部位相bit数CAN FDと専用100BASE-T1の内部実装、通知codecと公開拡張APIの境界を反映 |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版草案。CAN FD・100BASE-T1の実装契約と独立解析fixture |

文書ID：`design-original-network`

状態：採用した抽象profileの契約を確定。開発中ソースに製品実装を追加。製品試験の実施範囲は本書の検証記録又は対応する検証仕様を参照。規格全体適合・実機試験は未実施。文書版を1.1.2に統一し、公開版はv1.1.2向けPRとして準備する。独立解析fixtureと製品実行の証跡は区別する。

[architecture#arch-original-network](../アーキテクチャ設計書.md#arch-original-network)の分担を具体化する。仕様正本は[追加通信仕様](../specs/models/CANFD・100BASE-T1詳細機能仕様書.md)。共通Core/dispatcher/Context/journalの契約を変更しない。

<a id="canfd"></a>

## 1. FDの準備・状態・遷移

```trace
{
  "id": "design-original-network#canfd",
  "stage": "design",
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
    "architecture#arch-original-network"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 要素 | 実装契約 |
| --- | --- |
| 登録 | profile registryにcan.fd.precomputed.v1、generator registryにcan.fd.explicit.v1。ControllerV1/BusV1 descriptorはCCと同型のparameters/gates、BusのbitrateをnominalBitrate/dataBitrateへ置換。protocol=`dir.canfd`、tx message=`TxRequestV1` / schema=`dir.canfd.TxRequestV1`、rx message=`NotificationV1` / schema=`dir.canfd.NotificationV1`、schema版1（schema名はprotocol+"."+message）、Bus capability=`canfd-bus`版1。CC descriptorと別キーにする |
| prepare | common INI/NED → 型・peer・単一Bus → 速度 → 全generator/frame/wire厳密decode → DLC/ID/内容結合SHA-256/送信者一意 → 不変frame cacheの順に検査する。frame_idはgenerator id、request_idはgenerator id+":"+ordinalの衝突しない共通escapingを使用する。違反をdetails.rule/target付きE-0001に集約してinitialize前に終了する |
| FrameCache | prepareでdataをlowercase、整数を先頭0なし十進へ正規化し、format/id/data/brs/N/D/Rn/RdのASCII pipe結合SHA-256を照合する。evidence自体はhash fieldへ加えず文字列を保存する。format/id/data/DLC/brs、N/D、evidence/binding_sha256、Rn/Rd、checked duration/combined occupancyを保持。duration分子はchecked u128、ceil(a/b)はa/b+(a%b!=0)で加算overflowを避ける。最終時刻u64を検査する。汎用channelがFD bit数を計算する処理は対象外（注） |
| 状態 | BusState={current request?,release token?,dirty}、ControllerState={ready優先queue}、RequestMap={immutable input,generation,committed timestamps,state}、ReceptionMap={request/receiver,arrival?,completed?,state}。ready queue順は仲裁列→generated→generator id→ordinal。初期current=null/queue空 |
| Generate/Ready | phase1でpending行とready timerを作成、ready時に容量を検査しqueued又はdroppedへ遷移。txProcessingDelay=0なら同phase1の生成batch内でreadyまで処理し同時刻phase2へ候補を公開。正delayはfuture phase1 callback。複数readyの順は安定した生成順 |
| Arbitrate | phase2で全端候補snapshotを比較し勝者1件をqueueからcurrentへ。SOF/eof/releaseと全受信予約に必要な算術・schemaを検証後一batchでcommit。Bus内部coordinatorのみ当該Busの解決済output handleへ配送権限を持つ |
| EOF/Release | phase0 EOFでserializedと正常送信量をcommitし、各非送信ControllerへのNotificationV1 Arrivalをsend_at(EOF+source tx delay+receiver rx delay)する。受信参照はimmutable request_id/generationを用いる。Releaseでcurrent=null、dirty=true。ArrivalでrxFilter不適合ならfiltered、適合ならprocessing、rxProcessingDelay=0はそのbatchでcompleted、正delayはphase0 completion timer |
| journal | schema2追加行をModelRecordとしてappend/update deltaへ格納。各callbackで全予約・レコードを検証してからcommitする。失敗後は既存journal prefixを出す。停止時は予定時刻から架空EOF/完了を補完せず最後の状態を投影する |
| 検証 | FD4byte、N30/D80、Rn500000/Rd2000000のsynthetic入力は100000000ps、IFG6000000ps。N/Dの真偽はこの層で検証できないためwire_validationをstructural-only固定とする |
| binding観測点 | [TEST0097](../verification/cases/CANFD・100BASE-T1検証仕様書.md#dir-test-0097)で正規化8field、結合bytes、hash比較、checked時刻、公開済frame行を採取。payload/N/D/Rn/Rdの独立改変はhash保持ならprepare失敗、再結合した構造適合入力は受理。全prepare成功前のFrameCache又は静的ModelRecordを公開しない |
| フィルタ観測点 | [TEST0098](../verification/cases/CANFD・100BASE-T1検証仕様書.md#dir-test-0098)でEOFの受信行作成、Arrival判定とtimer所有を採取。不適合はarrivalだけcommitしてfiltered、completed=null、planned_completedは適合仮定の値を保持。completion timerは予約しない。適合だけprocessing/completedへ進みcompleted commitでreceivedを一つ増やす。ideal ACK/serializedは受信filterと独立 |
| 計測 | 登録はcanfd.generated/serialized/dropped/receivedの4 descriptor。EOFは送信者及び$allのserializedを一回、適合する受信者のcompletedはその受信者及び$allのreceivedを一回増やす。filteredはreceived/dropへ加えない。fidelity/evidence/hash/速度とwire_validationは不変cacheから結果へ写す |

<a id="t1"></a>

## 2. 100BASE-T1 adapter

```trace
{
  "id": "design-original-network#t1",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0182",
    "DIR-REQ-0183",
    "DIR-REQ-0184",
    "DIR-REQ-0185"
  ],
  "upstream": [
    "architecture#arch-original-network"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 要素 | 実装契約 |
| --- | --- |
| 登録と互換 | profile registryにethernet.l2.100base-t1.v1を追加する。既存Ethernet媒体実装の共通full pipelineへ、独立したProfilePolicy {allowed_phy_mode:100base-t1, bitrate:100000000, duplex:full, role_pair:master/slave}を渡す。既存v2 validatorの許可集合を拡張せず、両profileが同じ時間/queue/Frame/Attempt/Arrival部品を使用する。runtime coordinator IDには選択profileを含める。 |
| 準備とpipeline | prepareはv2構造検査→専用Policy→全方向速度→role/遅延→frame cacheの順。全PHY値とpropagationをchecked u128加算する。PairStateの方向別MacStateが独立FIFO/current/releaseを所有し、共通fullのphase0 EOF/Arrival、phase1 offer、phase2方向別SOFを使用する。roleを優先度や送信許可へ使わない。AttemptMap行の不変generationでArrivalを照合し、次SOFのport現在世代とは比較しない。受信処理後のswitchcopyは通常v1/v2 FDB経路へ進む。 |
| 結果と検証 | phy_link/profile metadata以外のレコードschemaはv2と共通。T境界でEOF/release/Arrivalが未commitなら予定値と実績を区別する。旧v1/v2入力を旧profileで実行した結果に追加profile由来の行を挿入しない。検証は[TEST0042/0043](../verification/cases/CANFD・100BASE-T1検証仕様書.md#dir-test-0042)。 |

## 開発中実装の境界

製品コードは共通INI/NED・結果公開層に専用input/runtime/outputを接続する。媒体対の論理timeline・予約世代・試行状態、CAN FDの不変frame cacheと受信状態をモデル内部で所有し、callbackの時刻・予約を事前検証してから結果へ反映する。公開ProfileRegistry、汎用Context/Envelope/TimerToken APIとモデル共通arenaは未実装であり、上記設計の公開拡張APIへの適合完了を示さない。

CAN FDは外部位相bit数による時間評価で、結果に`externally-precomputed-phase-bits`と`structural-only`を保存する。媒体v2は連続idle96bitのdeferenceと固定SHA-256 BEB、T1は時刻0からlink-up済みの固定PHY遅延を採用する。内容依存FD CRC/stuffing、波形、training、規格全文適合はこの実装の範囲外。
