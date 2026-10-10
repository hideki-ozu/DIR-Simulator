# CANモデル詳細機能仕様書

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | Busゲート名の自由化と旧形式入力の互換性を反映。文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。採用する規則・境界・拡張契約を確定。payload・イベント遷移・初期状態を具体化し、詳細設計・検証入力へ接続 |

文書ID：`spec-can-models`

| 項目 | 内容 |
| --- | --- |
| 文書状態 | 仕様決定済み。注：実装・検証の合格判定は本書の状態の対象外。CANの詳細設計と検証ケースは末尾リンクから参照し、共通基盤を含む全分担の追跡状態は一覧で管理する |
| 対象 | [CAN通信](../../機能仕様書.md#dir-func-0023)と[送信負荷](../../機能仕様書.md#dir-func-0027)。現行版は単一CAN CC共有バスの理想通信プロファイル |

## 要件の分担と通信全体の確認

0.1.0の[CAN性能評価](../../要件定義書.md#dir-req-0001)をモデル側で具体化する親要件は[CAN通信の成立](../../要件定義書.md#dir-req-0058)である。子要件の入力・送信・受信を一つの要求の経過としてつなぎ、非競合・競合・過負荷で待ちと性能の変化を説明できることを[DIR-AC-0009](../../要件定義書.md#dir-ac-0009)で確認する。以下は既存契約の確認順序であり、個別の規則は参照先の節を正本とする。

| 確認段階 | モデル固有の分担 | 共通基盤との接続 |
| --- | --- | --- |
| 構成・負荷を確定する | [profile](#profile)で単一Busと接続対を固定し、[frame-input](#frame-input)で形式・内容・送信者所有を検証。[workload](#workload)で周期と明示列を合わせ、request_idごとの生成時刻を決める | NEDとINIが構造・値・参照元を解決する。T以後の入力も検証し、実生成数にはT未満の発火だけを含める |
| readyから送信権を得る | [controller](#controller)でTX処理と有限待機キューを適用し、[arbitration](#arbitration)でノード内候補とバス全体の勝敗を分ける。敗者を保持し、進行中の送信は継続する | 実行基盤の同時刻phase順が解放・生成・仲裁の参加集合を確定する。容量は待機件数、占有はBusの時間区間として資源・計測へ渡す |
| 送信・受信・終了を照合する | [wire-time](#wire-time)でCRC/stuffを含むEOFとreleaseを計算し、[ack-errors](#ack-errors)と[controller](#controller)で送信成功・受信者別状態を確定する | 結果仕様が要求・受信・busy区間を別母数で集計する。[0,T)と異常時の確定prefixは実行・診断仕様を適用する |

例えば、競合する二要求を比較するときは生成時刻だけで送信順を判定せず、TX処理後のready、ノード内候補、SOFから、EOF後のreleaseと受信完了をそれぞれ追う。キュー満杯はreadyでの破棄、仲裁敗北は待機継続、EOF到達は送信成功、フィルタ拒否は受信者側の終端であり、同じ失敗件数へまとめない。EOFがTと等しければ未完了、EOFがT未満でも受信完了がT以上なら送信成功と受信未完了をともに残す。

親要件の確認は[CAN詳細設計](../../design/CANモデル詳細設計書.md)の状態・payloadと[CAN検証仕様](../../verification/cases/CANモデル検証仕様書.md)の独立期待値を、共通実行・資源・結果の分担と合わせて行う。[計算例](#examples)や個々の子要件の一致だけで通信全体の実装合格とは扱わない。

<a id="profile"></a>

## 1. 採用プロファイルと拡張境界

```trace
{
  "id": "spec-can-models#profile",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0033",
    "DIR-REQ-0058",
    "DIR-REQ-0097",
    "DIR-REQ-0114",
    "DIR-REQ-0001"
  ],
  "upstream": [
    "DIR-FUNC-0006",
    "DIR-FUNC-0011",
    "DIR-FUNC-0012",
    "DIR-FUNC-0013",
    "DIR-FUNC-0020",
    "DIR-FUNC-0023"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 採用規則 |
| --- | --- |
| プロファイル | `can.cc.ideal.v1`。CAN CCの11bit/29bitデータフレーム、仲裁、内容依存時間、同報、理想ACKを再現する |
| 構造 | `dir.can.Controller`がキュー・負荷・受信を担当、`dir.can.Bus`が仲裁・転送を担当、`dir.link.FixedDelay`が接続ごとの観測遅延を担当。[NED仕様](../NED詳細機能仕様書.md)の型登録・ポート契約で結合する |
| 接続 | Controllerの出力tx→Busのinput、同じBusのoutput→同Controllerの入力rxを1組とする。v1.0.0ではBusゲートを任意のNED識別子で宣言し、入出力の対は接続経路から決定する（[NED仕様](../NED詳細機能仕様書.md#connections)）。旧命名も受理する。Busは1個、Controllerは2個以上。全Controllerは常時active、同じbitrateで同期しているという前提を置く |
| 注：現行対象外 | remote frame、CAN FD/XL、レジスタ、トランシーバ電気特性、ビット時刻の同期補正、動的接続、エラー状態遷移、故障注入、確率負荷。対応を装う既定値へ変換せず、指定時は未対応入力として準備失敗 |
| 忠実度 | ビット列からビット数を算出するが、実行イベントはフレーム単位。注：電圧波形や各ビットイベントの生成、実機・規格全体への適合認証は対象外 |
| 拡張登録 | profile、generator kind、message typeを名前空間+版で登録する。各登録は入力型、検証、候補比較、時間計算、状態通知の契約を持つ。未知名・同名二重登録・非互換版は準備失敗 |
| 共通基盤 | コアは型付きpayloadと資源/インスタンスIDを扱う。注：スケジューラや汎用channelへのCAN format、CAN ID、DLC、CRCの固定は対象外。別モデルは同じ時刻・配送・計測契約を再利用する |
| 次版への接続 | bus IDはインスタンスパスでバスローカルに保持する。将来の複数バス/GWは独立仲裁と要求の親子関係を持たせ、別送信に新しいrequest_idを割り当てる。現行入力の複数バスは準備失敗 |
| 再現性 | profileの版、確定パラメータ、入力内容、generator順を結果に保存。同じ条件は同じ時刻・連番・状態を生成。確率分布と乱数seedの指定は準備失敗とする。将来generatorの版で導入する |

<a id="frame-input"></a>

## 2. フレーム入力と検証

```trace
{
  "id": "spec-can-models#frame-input",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0098",
    "DIR-REQ-0099",
    "DIR-REQ-0100",
    "DIR-REQ-0114"
  ],
  "upstream": [
    "DIR-FUNC-0007",
    "DIR-FUNC-0008",
    "DIR-FUNC-0009",
    "DIR-FUNC-0015",
    "DIR-FUNC-0021",
    "DIR-FUNC-0023"
  ],
  "state": "confirmed",
  "pending": []
}
```

| フィールド | 型・必須・範囲・意味 |
| --- | --- |
| format | 必須string、`standard`又は`extended` |
| id | 必須JSON整数。standardは0～2047、extendedは0～536870911。負値、小数、文字列、範囲外は拒否 |
| data | 必須の偶数桁hex文字列。0～16桁、各桁0–9/A–F/a–f、空文字列は0byte。空白・0x接頭辞・奇数桁を拒否。byte列順に送信し各byteはMSB先行 |
| DLC | dataのbyte数Dから0～8を導出。注：独立したDLC入力は対象外。payloadは指定されたbyte列をそのまま使用する |
| 固定ビット | DATA frameのRTR=0、standard IDE=0/r0=0、extended SRR=1/IDE=1/r1=0/r0=0。ACK/CRC/予約bitの利用者上書きキーは準備失敗とする |
| bitrate | Busの必須値。整数bps換算で1～1000000。0・非整数bps・上限超過を準備失敗とする |
| 未知キー | frameオブジェクトの未知フィールドと重複JSONキーを準備失敗とし、原因フィールドを診断する |
| 送信者所有 | 実行全体で各`(bus_id,format,id)`を送信するControllerは1個。全generatorを準備時に検査し、同じ組を異なる送信者が使えば、時刻が重ならなくても拒否する。同じ送信者の繰返し・複数generatorは受理する |
| 所有制限の理由 | 同じ仲裁列の異なるpayloadや同時送信はビット競合・ACK・エラーの忠実な処理が必要。本ideal profileは所有者一意という入力前提で結果の曖昧さをなくす |
| 受理例 | `{"format":"standard","id":291,"data":"00ff"}`は2byte DATA。`{"format":"extended","id":291,"data":""}`は別formatの0byte DATA |
| 拒否例 | standard id=2048、data=`"0"`、9byte data、`rtr=true`、`dlc=15`、他Controllerによる同じstandard id=291を拒否する |
| 参照版の注意 | 数値上の全11/29bit領域を本profileの受理範囲として明示する。注：Bosch 1991資料の旧予約ID制約は現行規格適合を証明する資料としての対象外。ISO 11898-1:2024本文の網羅照合は未実施 |

<a id="arbitration"></a>

## 3. ローカル候補とバス仲裁

```trace
{
  "id": "spec-can-models#arbitration",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0040",
    "DIR-REQ-0041",
    "DIR-REQ-0046",
    "DIR-REQ-0104",
    "DIR-REQ-0105",
    "DIR-REQ-0106",
    "DIR-REQ-0107",
    "DIR-REQ-0108"
  ],
  "upstream": [
    "DIR-FUNC-0012",
    "DIR-FUNC-0016",
    "DIR-FUNC-0017",
    "DIR-FUNC-0023"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 採用規則 |
| --- | --- |
| 仲裁列 | standardは`ID[10:0],RTR=0,IDE=0`、extendedは`ID[28:18],SRR=1,IDE=1,ID[17:0],RTR=0`。各ID区間をMSBから比較する。最初の相違で0が1に勝つ。優先度キーは非stuff列を用いる |
| ローカル候補 | Controllerはready済み待機キューから仲裁列が最小の1件を選ぶ。同じ仲裁列なら生成順（generated時刻、generator IDのUTF-8辞書順、generator内ordinal）で先の要求を選ぶ。優先要求は後着でも候補として選出できる |
| バス参加 | [実行順序](../実行詳細機能仕様書.md#event-order)のphase 2直前に各Controllerの候補を集める。注：未ready、処理中、SOF後の後着は当該参加集合の対象外 |
| 勝敗 | 仲裁列を比較し1件を選ぶ。所有者一意検証により別Controller間で仲裁列を一意にする。検証後の等値発生はモデル不変条件違反として実行失敗にする |
| 敗者 | キュー内に保持し、次の解放で優先度を再評価する。新着高優先度が次回候補に入る。候補は各調停時点のキューから選び直す。敗者も勝者の受信対象になる |
| 仲裁の時間 | 仲裁はSOF開始時に追加時間0で論理選出する。実際の仲裁フィールドbitは勝者フレーム長に1回だけ含める。送信量は勝者フレームの量のみとする |
| 混在例 | standard 0x123とextended 0x048C0000は先頭11bitが同じでstandardがRTR=0で勝つ。extended 0x00000123は先頭11bit=0なのでstandard 0x123より先。注：29bit数値と11bit数値の単純比較は対象外 |
| 同一ID繰返し | 同Controllerから時刻0に同一format/idの2件ならgenerator ID/ordinalで先の1件を送信し、残りを次回候補に保持。別Controllerなら準備時拒否 |

<a id="wire-time"></a>

## 4. CRC・スタッフィング・転送時刻

```trace
{
  "id": "spec-can-models#wire-time",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0034",
    "DIR-REQ-0036",
    "DIR-REQ-0037",
    "DIR-REQ-0058",
    "DIR-REQ-0098",
    "DIR-REQ-0099",
    "DIR-REQ-0100",
    "DIR-REQ-0108",
    "DIR-REQ-0109",
    "DIR-REQ-0110",
    "DIR-REQ-0111",
    "DIR-REQ-0114"
  ],
  "upstream": [
    "DIR-FUNC-0009",
    "DIR-FUNC-0012",
    "DIR-FUNC-0013",
    "DIR-FUNC-0015",
    "DIR-FUNC-0017",
    "DIR-FUNC-0021",
    "DIR-FUNC-0023"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 段階 | 採用する計算 |
| --- | --- |
| standardのCRC入力 | `SOF=0`、11bit ID、RTR=0、IDE=0、r0=0、4bit DLC、dataの順。非stuff状態で19+8D bit |
| extendedのCRC入力 | `SOF=0`、ID上位11bit、SRR=1、IDE=1、ID下位18bit、RTR=0、r1=0、r0=0、4bit DLC、dataの順。非stuff状態で39+8D bit |
| CRC-15 | 生成多項式`x^15+x^14+x^10+x^8+x^7+x^4+x^3+1`（下位係数0x4599）。CRC入力列を多項式Mとして`M*x^15`を生成多項式でGF(2)除算した15bit余りをMSB先行で付加。初期値0、反転・最終XORなし。CRC入力は非stuff列のみとする |
| 等価計算 | 15bitレジスタc=0から各入力bit bについてf=b XOR cの最上位bit、cを1bit左シフトし15bitへマスク、f=1なら0x4599をXORする。入力末尾でcがCRC。注：レジスタ方式への追加15ゼロbitの投入は対象外 |
| 動的stuff | SOFからCRC最終bitまでを連続して走査し、実送信列に同じbitが5個続いた直後へ反対bitを1個挿入する。挿入bitも次の連続数へ含め、フィールド境界でも連続数を保持する。CRC最終bitが5個目ならその直後もstuffを挿入する |
| 後続固定領域 | CRC delimiter=1の1bit、ACK slot=0の1bit（理想ACK後のバス値）、ACK delimiter=1の1bit、EOF=1の7bit。この10bitはそのまま付加する |
| フレーム長 | stuffing数SとしてSOF～EOFの`frame_bits=44+8D+S`（standard）又は`64+8D+S`（extended）。payload_bits=8D。総占有bits=`frame_bits+3` |
| intermission | EOF後にrecessive 3bit。注：追加overload/suspendは対象外。フレーム長・payload量とは別にbusy時間へ算入する |
| タイムスタンプ | SOF=t_s、EOF=t_s+ceil(frame_bits*10^12/R)、release=t_s+ceil((frame_bits+3)*10^12/R)。EOFでtransmitting→intermission、releaseでidle。新しいSOFはrelease以後 |
| 受信観測 | t_observed=t_EOF+sourceのtx channel.delay+receiverのrx channel.delay。経路遅延は完了フレーム通知をずらす。本profileの仲裁とACKは伝搬0の集中バスとして計算する。注：観測遅延を用いたACK期限や伝搬による仲裁競合は対象外 |
| 処理遅延 | t_ready=t_generated+txProcessingDelay。フィルタ適合時t_received=t_observed+rxProcessingDelay。処理は要求ごと独立、送信・受信並行可。待ち時間=t_SOF−t_ready。総受信遅延=t_received−t_generated |
| 出力内訳 | [結果仕様](../結果詳細機能仕様書.md#result-schema)のRequest.model_fieldsへprofile、crc15、stuff_bits、frame_bits、intermission_bits=3、bitrate_bpsと予定/到達時刻を保存する。frame_bitsはRequest.serialized_bitsと一致。payload_bitsはRequestに保存する。planned_eof_ps/planned_release_psはSOF到達後の予定、eof_ps/release_psは実到達後のみとする。受信時刻はReceiverに保存する |
| 計算確認 | 整数算術の許容差は0ps。実数の理想時間との差のみ[時刻仕様](../実行詳細機能仕様書.md#time)の1ps未満切上げを認める。注：外部実機との差の精度保証は対象外 |

<a id="ack-errors"></a>

## 5. ACK・成功・エラーの前提

```trace
{
  "id": "spec-can-models#ack-errors",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0058",
    "DIR-REQ-0108",
    "DIR-REQ-0110",
    "DIR-REQ-0111",
    "DIR-REQ-0113",
    "DIR-REQ-0114",
    "DIR-REQ-0115"
  ],
  "upstream": [
    "DIR-FUNC-0012",
    "DIR-FUNC-0013",
    "DIR-FUNC-0017",
    "DIR-FUNC-0018",
    "DIR-FUNC-0019",
    "DIR-FUNC-0020",
    "DIR-FUNC-0023"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 条件 | 結果・前提 |
| --- | --- |
| ACK成立 | 送信者以外に同Busへ正しく接続されたactive Controllerが1個以上あればACKが成立する。全Controller常時active、エラーなしを準備検証で固定する |
| フィルタとの分離 | rxFilterがnoneのControllerも正しいフレームへACKする。アプリケーションの受理件数とACK成立を独立して判定する |
| ノード1個／相手不在 | 負荷の有無にかかわらず準備失敗。注：自己ACKや成功の擬装、無限再送は対象外 |
| 送信成功 | ACK成立を前提にEOFイベントが実行された時点で1要求の成功を確定。注：SOF時点・途中の受信観測予定は成功判定の対象外 |
| 送信試行 | SOFを開始した要求のattempt_count=1、未開始は0。試行数はSOF開始回数のみを数える。注：初期版は通信エラーによる試行結果の生成が対象外 |
| 通信エラー | 注：CRC不一致・bit/stuff/form/ACK error、error frame、overload frameの発生は対象外。CRCは時間計算のため生成し、外部破損bitstreamの検査・エラー注入の入力は準備失敗とする |
| 状態 | 全Controllerはideal-active固定。error-passive、bus-off、TEC/REC、復帰、再送設定を入力した場合は未対応として準備失敗 |
| 拡張方法 | エラーを扱う将来profileは試行ID・要求IDを分け、ACK結果、エラー占有区間、再送待ち、状態遷移を型付き通知で追加する。初期profileの成功数/試行数の意味を保持する |
| 適用範囲 | 結果は理想通信下の仲裁・負荷・待ち・内容依存利用率の評価。注：故障耐性、物理配線長限界、再送による遅延上限の評価は対象外 |

<a id="workload"></a>

## 6. 負荷JSONと要求識別

```trace
{
  "id": "spec-can-models#workload",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0024",
    "DIR-REQ-0045",
    "DIR-REQ-0101",
    "DIR-REQ-0102",
    "DIR-REQ-0103",
    "DIR-REQ-0115",
    "DIR-REQ-0006",
    "DIR-REQ-0007"
  ],
  "upstream": [
    "DIR-FUNC-0001",
    "DIR-FUNC-0007",
    "DIR-FUNC-0009",
    "DIR-FUNC-0012",
    "DIR-FUNC-0016",
    "DIR-FUNC-0018",
    "DIR-FUNC-0020",
    "DIR-FUNC-0023",
    "DIR-FUNC-0027"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 採用規則 |
| --- | --- |
| 読込元 | INI `[General] workload="workload.json"`。相対パスはINIの親を基準。未指定は空generator配列。UTF-8 JSONの重複キー・未知キー・型不一致・未知対象を準備失敗とする |
| ルート | `{"schema_version":1,"generators":[...]}`。schema_versionは整数1のみ。generatorsは必須配列で空を受理。未知キーは準備失敗として診断する |
| 共通generator | 必須`id`（`[A-Za-z_][A-Za-z0-9_]*`で全体一意）、`kind`、`node`（正規Controllerパス）、`frame`（第2節）。generator IDの辞書順が同時刻生成順になる |
| 明示時刻列 | kind=`can.explicit.v1`、必須`times`は時間文字列配列。非減少順、同時刻重複可、空可。各要素を1要求としordinal=0始まりの配列index。負時刻・非整列を拒否。T以上の要素も妥当性検査し、区間外予定として保持する |
| 周期 | kind=`can.periodic.v1`、必須`start`・`period`、任意`phase`既定0、任意`end`・`count`。start>=0、period>0、0<=phase<period、end>=start、countは0～u64max整数 |
| 周期の式 | ordinal k=0から`t_k=start+phase+k*period`、t_k<end（指定時）かつk<count（指定時）の範囲で発火。end/countの両方を指定すれば両方を満たす。両方省略時は論理的な無限列とし、実際の生成はT未満に限定する。T以後の候補は終了理由time_limitの判定に使用する。startとendは実行原点基準の絶対相対時刻 |
| 生成境界 | 実際の生成はt_k<T。phaseにより最初の発火がend以上なら0件。count=0も0件。period=0は準備失敗とする |
| 予約・オーバーフロー | 発火候補がend/T境界外と厳密に判定できる場合は物理予約を省略する。必要なt_kを十分広い整数で計算し、表現不能な発火を予約する場合は時間算術失敗。注：全期間の巨大配列への事前展開は要求対象外 |
| 混在 | 全generatorを遅延展開するdispatcherは、次の最小発火時刻について同時刻の全発火を収集してから予約する。前の予約時期にかかわらず同じ順序を保持する。同じバス上でperiodicとexplicitを混在できる。同時刻生成はgenerator ID順、同一generator内ordinal順 |
| 要求ID | 実行内の`request_id="<generator_id>:<ordinal>"`。別実行はrun_idで区別。format/idが同じ繰返しでも別要求。全観測・破棄・受信にはrequest_idとbus_id/source_idを保持する |
| 確率負荷 | 初期版は決定的generatorのみ。random/seed/distributionキーは準備失敗。将来は新kindの入力スキーマに分布、PRNGの版、seed、独立stream IDを必須化し、既存generatorの順序を保持する |
| 受理JSON例 | `{"schema_version":1,"generators":[{"id":"p","kind":"can.periodic.v1","node":"Main.a","start":"0ps","period":"1ms","phase":"0ps","count":2,"frame":{"format":"standard","id":0,"data":""}},{"id":"x","kind":"can.explicit.v1","node":"Main.b","times":["0ps","1ms"],"frame":{"format":"extended","id":1,"data":"ff"}}]}` |
| 例の期待 | T>1msならp:0とx:0が0、p:1とx:1が1msに生成。T=1msなら:0のみ生成。本例をworkload.jsonとして保存し、NED仕様のdemo.Mainと設定仕様のINIのGeneralへworkload="workload.json"を追加して使用する。実装済みfixtureではない |

<a id="controller"></a>

## 7. パラメータ・受信・要求状態

```trace
{
  "id": "spec-can-models#controller",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0035",
    "DIR-REQ-0038",
    "DIR-REQ-0039",
    "DIR-REQ-0101",
    "DIR-REQ-0103",
    "DIR-REQ-0104",
    "DIR-REQ-0107",
    "DIR-REQ-0111",
    "DIR-REQ-0112",
    "DIR-REQ-0113",
    "DIR-REQ-0115",
    "DIR-REQ-0120"
  ],
  "upstream": [
    "DIR-FUNC-0012",
    "DIR-FUNC-0013",
    "DIR-FUNC-0014",
    "DIR-FUNC-0016",
    "DIR-FUNC-0018",
    "DIR-FUNC-0019",
    "DIR-FUNC-0020",
    "DIR-FUNC-0023",
    "DIR-FUNC-0027"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 規則 |
| --- | --- |
| パラメータ | queueCapacity:int既定64、txProcessingDelay:time既定0ps、rxProcessingDelay:time既定0ps、rxFilter:string既定`"*"`。容量・遅延の値域は[資源仕様](../資源詳細機能仕様書.md#queue)に従う |
| フィルタ構文 | `*`は全て、`none`は全拒否。それ以外は`std:0xHEX`又は`ext:0xHEX`のcomma区切り集合。HEXは1～8桁でframeの範囲内、空白・重複項目・空項目は拒否する。IDの表記大小文字は数値へ正規化する |
| 判定 | EOF後の観測時に(format,id)が完全一致すれば適合。注：マスク・式・動的変更は初期版対象外。フィルタはACK条件とは独立 |
| 自己配送 | 注：source Controller自身へのアプリ受信通知は対象外。他の全Controllerへ観測通知を1個ずつ作る。仲裁敗者も受信できる |
| 受信状態 | EOFの実処理で送信元以外の受信行を作成する。受信行はEOF以後に存在する。観測前はpending、観測時に不適合ならfilteredで終了、適合ならpendingのまま受信処理を開始し、rxProcessingDelay後にreceived。receiver_id別に保持し、filteredを要求のdropや通信失敗とは別集計とする |
| 要求状態 | generatedと同時にprocessingへ入り、t_readyで容量判定後pending又はdropped。仲裁でpending→in_flight、EOFでin_flight→success。dropped/successは終端。Tで残るprocessing/pending/in_flightは未完了 |
| 受信の終端 | success要求でも各受信通知がT以上なら受信未完了となる。受信者が複数でも送信成功は1件、受信完了はreceiver別件数。アプリ受信0件でも成功は成立する |
| 計算確定時点 | 不変のframeは準備時に直列化を計算・cacheし、generatedのcommit時に各Requestへ全CanFieldsとserialized_bitsを設定する。予定時刻はSOFのcommit時に設定。注：準備計算は要求生成数・送信量に算入しない |
| メッセージ型 | txは登録型`can.cc.ideal.v1.CanTxRequest`、rxは`can.cc.ideal.v1.CanNotification`、いずれもschema版1。通知はrequest_id、source_id、receiver_id、bus_id、profile、typed frameを保持。制御通知・受信配送・Timerを型で区別し、frameの内部項目の解釈をモデルが担当する |
| payloadの管理元 | 下表をポートpayloadの正本とし、[詳細設計のpayload](../../design/CANモデル詳細設計書.md#payloads)がdecoder・内部通知を実現する。容量判定はCanTxRequest受理時に行う |
| bytes codec | 登録schemaは`can.cc.ideal.v1.CanTxRequest`と`can.cc.ideal.v1.CanNotification`、version=1。payloadは以下のキー順でcompact UTF-8 JSONに直列化する。未知・欠落・重複キー、非正規数値、型不一致は入力拒否。Envelopeの時刻・対象とpayloadの意味をCAN decoderが照合する |
| 共通型 | `D`=先頭0なし非負10進文字列（0は`"0"`）、`P`=解決済みインスタンスパス、`F`=CanFrame、`I`=request_id。全キー必須。内部時刻とordinalはchecked u64。enumは記載した大小文字を保持 |
| CanFrame | `{"format":"standard","id":0,"data_hex":""}`。この順のキー。formatはstandard/extended、idはJSON整数、data_hexは入力dataを小文字へ正規化したhex。DLCはdata_hexから導出する |
| CanTxRequest | `{schema_version:1,profile:"can.cc.ideal.v1",request_id:I,source_id:P,bus_id:P,frame:F,generated_ps:D,ready_ps:D}`。ready_psがEnvelope時刻と一致、宛先bus、sourceがtx経路のControllerと一致。TX処理完了後の制御通知として扱う。注：channelのdelay加算は対象外 |
| CanNotification | `{schema_version:1,profile:"can.cc.ideal.v1",request_id:I,source_id:P,receiver_id:P,bus_id:P,frame:F,generated_ps:D,sof_ps:D,eof_ps:D,observed_ps:D}`。宛先receiverとEnvelope時刻observed_psを照合。frameは送信frameと同じ値。txとrx経路のdelay合計はobserved_psへ適用済み |
| 初期状態 | [詳細設計の初期状態](../../design/CANモデル詳細設計書.md#state-machine)を結果metadata.initial_stateへ格納する。初期キュー・処理中集合・受信集合は空、バスidle、生成器cursorは先頭とする |
| 終了集計 | generated=success+dropped+未完了processing+未完了pending+未完了in_flight。generatedは実際の発火要求のみを数える。受信状態の保存と各指標の出力は[結果仕様](../結果詳細機能仕様書.md)を正本とする |

本節は既定ideal profileと通常Controllerの送信規則を定める。multibusのGW portは[GW転送仕様](GWモデル詳細機能仕様書.md#forwarding)のRX保持と正容量TX満杯時のwaiting_txを適用する。GWのRX容量は受信完了した親フレームの転送保持であり、通常ControllerのアプリケーションRXキュー・CPUサービスモデルは対象外（注）。

<a id="examples"></a>

## 8. 正常・境界・独立計算例

```trace
{
  "id": "spec-can-models#examples",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0045",
    "DIR-REQ-0046",
    "DIR-REQ-0099",
    "DIR-REQ-0100",
    "DIR-REQ-0104",
    "DIR-REQ-0105",
    "DIR-REQ-0106",
    "DIR-REQ-0109",
    "DIR-REQ-0110",
    "DIR-REQ-0112",
    "DIR-REQ-0113",
    "DIR-REQ-0114",
    "DIR-REQ-0115",
    "DIR-REQ-0120",
    "DIR-REQ-0001",
    "DIR-REQ-0116"
  ],
  "upstream": [
    "DIR-FUNC-0001",
    "DIR-FUNC-0009",
    "DIR-FUNC-0012",
    "DIR-FUNC-0013",
    "DIR-FUNC-0014",
    "DIR-FUNC-0015",
    "DIR-FUNC-0016",
    "DIR-FUNC-0017",
    "DIR-FUNC-0018",
    "DIR-FUNC-0019",
    "DIR-FUNC-0020",
    "DIR-FUNC-0021",
    "DIR-FUNC-0023",
    "DIR-FUNC-0027"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 入力 | CRC hex | stuff数 | frame bits | intermission込みbits |
| --- | --- | --- | --- | --- |
| standard ID=0、D=0 | 0000 | 6 | 50 | 53 |
| standard ID=0x123、D=0 | 6858 | 1 | 45 | 48 |
| standard ID=0x123、data=0000000000000000 | 044E | 14 | 122 | 125 |
| standard ID=0x123、data=ffffffffffffffff | 6284 | 13 | 121 | 124 |
| extended ID=0、D=0 | 4610 | 7 | 71 | 74 |

| 確認条件 | 期待結果・証拠の限界 |
| --- | --- |
| 独立計算の手順 | 上表は本書の入力bit列について、シフトレジスタ方式とGF(2)整数多項式の長除算という2方式のCRCが一致することを作成時に確認した解析例。注：外部実機採取値・第三者配布golden vector・シミュレータ実行の合格判定は本例の証拠の対象外 |
| 手計算可能な0byte例 | standard ID=0のCRC入力19bitは全0、CRC15bitも全0。34個の元bitに5個ごとに反対bitを挿入してS=6、34+6+10=50bit、間隔込み53bit |
| 空payloadのbit列 | standard ID=0x123の非stuff SOF～CRC列は`0001001000110000000110100001011000`。field連続走査でstuffが1個、末尾固定10bitを足して45bit |
| 同時2要求 | A:standard ID0 D0、B:standard ID1 D0、両者t_ready=0、500kbps、遅延0。Aが先行してEOF100us、release106us。Bは106usの次回候補。Aの占有は53bit分で確定する |
| フィルタ全拒否 | A送信、B.rxFilter=none。AはEOFでsuccess、Bはobservedでfiltered。受信完了0、送信成功1、ACKあり |
| 容量ゼロ | queueCapacity=0で1件生成。TX処理完了時にdropped、SOFなし、attempt_count=0。負荷0件なら生成・破棄とも0 |
| ローカル優先 | 同Controllerにstandard ID10とID2がreadyならID2を候補にする。同IDの2件なら生成順。別Controller同IDは準備失敗 |
| 終了境界 | AのEOF100us、T=100usならin_flightで成功0。T=100us+1psならsuccess、intermission中で終了。bus busyは両条件とも観測区間末まで積分 |
| 遅延合成 | [資源仕様の115us例](../資源詳細機能仕様書.md#delay)で各成分を1回ずつ加算。t_observedやt_receivedがreleaseを越えても次SOFを開始できる |
| 下流確認 | [CAN詳細設計](../../design/CANモデル詳細設計書.md)と[CAN検証仕様](../../verification/cases/CANモデル検証仕様書.md)で具体化する。static fixtureとCRC二方式照合を提供する。注：シミュレータの実行合格は未確認 |

<a id="sources"></a>

## 9. 参照資料と決定根拠

| 資料 | 確認範囲・利用方法 |
| --- | --- |
| [ISO 11898-1:2024 公開書誌](https://www.iso.org/standard/86384.html) | 第3版2024-05、DLL/PCSの参照規格であることを確認。全文は取得・網羅照合していない。版の参照と規格適合の保証を区別する |
| [CiA: CAN CC](https://www.can-cia.org/can-knowledge/can-cc) | 11/29bit、同報、仲裁、DATA長、ACK、3bit intermission、stuffの公開説明を確認。モデルのactive固定・所有者一意・フィルタ構文は本プロジェクトの決定 |
| [CiA: CRC in CAN frames](https://www.can-cia.org/can-knowledge/cyclic-redundancy-check-crc-in-can-frames) | CAN CCの15次多項式とFD/XLとの差を確認。CRC値は本書入力から独立計算 |
| [Robert Bosch GmbH: CAN Specification 2.0 (1991), Part B](https://tech-tools.com/files/can2spec.pdf) | 原著者資料の第三者ホスト複製であることを区別。Part Bの3.2.1（frame/CRC/ACK）、3.2.5（intermission）、5（coding）を参照。配列・多項式・stuff範囲を確認。本文・図・ソースコードを転載せず、プロジェクトの計算契約を独自記述。メーカー現行URLは取得できなかった |
| 仕様判断 | キュー・観測遅延・生成形式・停止境界・理想ACKの採否は本プロジェクトの仕様決定。通信エラー再現の実装状態は未実装として区別する |
| レビュー状態 | DIR-TBD-0004/0005/0011/0012/0013/0014の採用内容は本書と実行・資源仕様で確定。共通基盤の下流未割当と将来profileの対象外は、現行仕様の判断保留とは区別する |
