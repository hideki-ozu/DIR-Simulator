# SoC・AHB・NoC詳細設計書

文書バージョン：`0.1.0`
対象GitHubバージョン：`未リリース（main @ 7738b55）`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版作成。SoC共有バス・AHB相当・NoCの抽象評価契約を追加 |

文書ID：`design-soc-models`

文書状態：契約確定。製品実装・製品実行試験は未実施。

担当元：[architecture#arch-soc-models](../アーキテクチャ設計書.md#arch-soc-models)。外部契約は[モデル仕様](../specs/models/SoC・AHB・NoC詳細機能仕様書.md)を正本とする。

| 項目 | 確定契約 |
| --- | --- |
| prepare | 共通NED/INI解析→profileキー/型→全配置とportの対応→clock/容量/対象range/mesh→全workload→空state構築の順。initialize成功時に空queue/cursor0/active nullをmetadata.initial_stateへcanonical JSONで保存する |
| event codec | profile+`.Generate`版1 phase1=`{generator_id:S,ordinal:D}`、`.Complete`版1 phase0=`{resource:S,request_id:S,hop:D}`、`.Wake`版1 phase0=`{context:S}`。全キー完全一致、Dはcanonical非負十進文字列、JSONはキーUTF-8辞書順compact bytes。共通Envelopeのsender/recipient/request/timeをstateと照合し、schedulerが管理するTimerTokenはcontextの予約所有表で照合する |
| adapter | 登録module adapterは型付きRequest/Response/Packet接続をcontextへ解決する。内部timerで両端を一batchに更新し、同じ論理転送の二重配送を防ぐ。外部payloadは`{request_id:S,source:S,target:S?,bytes:D}`、Requestは追加`{operation:S,address:D}`、Responseは追加`{response:S}`、Packetは共通だけ。profile/schema/版/送受pathをcontext内要求と照合する |
| 数値とcommit | 時刻・ceil・byte量をu128中間値で算出しu64へ検査する。遷移案、timer、journal、計測deltaを検証後に一effect batchでcommit。T以後の予定も表現検査してFESへ保持する。codec/state/overflow失敗は仕様の共通error分類へ変換する |
| journal | transaction生成/状態更新・grant時active_plan設定・完了時active_plan解除とtransfer完了行、queue/busy/標本・byte計測を同じbatchへ記録する。exportはjournalの確定prefixから作り直す。内部stateが部分変更された失敗batchを結果へ反映する処理は対象外（注） |
| 起床 | 最小future eligibleにcontext一件だけWakeを保持し、より早い予定で置換する。資源完了でdirtyを指定しphase2を依頼する。idleの全clockのtick生成は対象外（注） |


<a id="soc"></a>

```trace
{
  "id": "design-soc-models#soc",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0186",
    "DIR-REQ-0187",
    "DIR-REQ-0188",
    "DIR-REQ-0189"
  ],
  "upstream": [
    "architecture#arch-soc-models"
  ],
  "state": "confirmed",
  "pending": []
}
```

## 1. SharedBusContext

| 項目 | 確定契約 |
| --- | --- |
| 所有 | bus単位にsources FIFO、pending+active数、cursor、active、target decode表、要求台帳を所有する。各sourceのqueueとbus占有の積分器を保持する |
| Generate | capacity確認→drop又はFIFO append→transaction行→queue更新→eligible Wake/dirty。active中の新着もFIFOへ保持する |
| grant | cursor又は(priority,path)順にFIFO先頭を選び、decode/交差/長さから完了時刻を計算。active設定、pop、cursor更新、start記録、busy開始、Complete予約を一括commitする |
| Complete | token/active照合→OKAY/ERROR completed→source使用数減→busy解除→transfer/標本/byte→dirty。ERRORでも完了件数を一つ増す |
| 不変条件 | active<=1、source使用数=pending+active、FIFOはpendingだけ、completed+dropped+pending+active=generated。固定priorityとRRは同じ計算済候補表に適用する |


<a id="ahb"></a>

```trace
{
  "id": "design-soc-models#ahb",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0190",
    "DIR-REQ-0191",
    "DIR-REQ-0192",
    "DIR-REQ-0193"
  ],
  "upstream": [
    "architecture#arch-soc-models"
  ],
  "state": "confirmed",
  "pending": []
}
```

## 2. AhbContext

| 項目 | 確定契約 |
| --- | --- |
| 所有 | manager FIFO/capacityとcursor、activeのaddress/data/wait/error段階、target decode表をcontextへ保持する。外部時刻契約をSoCと共有するがAHBのcycle式を専用policyとする |
| 段階計画 | grant gでaddress_end=g+P、data_end=g+(2+wait)*Pを算出。ERRORは追加1cycle。外部可視差分のない段階は内部時刻として保持し、最終Completeだけを予約する |
| 停止 | Tがaddress/data/wait/error中でもactiveとbusyを維持する。最終transfer行、completed、delivered_bitsはComplete commitで初めて出力する |
| 仲裁・応答 | SoCのRR/capacity engineを利用し、4byte整列・単一beat・wait/error decoderをAHBに限定する。非decodeERRORはwait0、decodeERRORはtarget設定waitを使用する |


<a id="noc"></a>

```trace
{
  "id": "design-soc-models#noc",
  "stage": "design",
  "requirements": [
    "DIR-REQ-0194",
    "DIR-REQ-0195",
    "DIR-REQ-0196",
    "DIR-REQ-0197"
  ],
  "upstream": [
    "architecture#arch-soc-models"
  ],
  "state": "confirmed",
  "pending": []
}
```

## 3. MeshContext

| 項目 | 確定契約 |
| --- | --- |
| 所有 | MeshContextに座標→router、endpoint→router、source FIFO、入力FIFO、下流予約slot数、出力別active/cursor、packet hop、台帳を保持する |
| 二段階調停 | まずsource注入をpath順に実行。次に出力resource順でFIFO headのXY方向・eligible・slotを検査しRR grant。注入又はgrantがあればsource注入から再走査し、同edge内の空slot伝播を解決する。各outputは一度しかidleからbusyへ移れないのでgrantを伴う走査は出力数以下であり、追加の注入だけの走査と最後の固定点検査を含め出力数+2以下 |
| reservation | grant時にpacketを入力からpopして出力へ移し下流reserved++。Completeでreserved--、下流FIFOへappend。packetの唯一所有者はsource FIFO/input FIFO/active output/終端台帳の一つ。予約slotはpacket本体の複製を持たない |
| phase0順序 | 完了予定をcontext内min-heapへ格納し、最小予定時刻ごとにcontextのComplete dispatcher timerを一件だけ予約する。timer callbackは同時刻予定を全件取り出して(resource ID,request_id)順に一batchへまとめて適用する。内部Complete payloadは代表予定を示し、callbackが同時刻cohort全体を処理する。異なる予約の容量とFIFO順序を安定化し、全完了後にphase1/phase2へ進める |
| routing | next_portはx差→y差→localだけを返す。配置検査で正方向の隣接routerが必ず存在することを確認する。最初のgrantでstart_psを保存し、最後のlocal Completeでcompleted/OKAYとする |
| gauge | queueからpop/append時に待機件数積分と最大を更新。reservedをqueue_meanへ混入させず、capacity検査には必ず加える。出力busyと最終配送量を別に計測する |
| progress | 固定点後に未完了・全idleなら最小future eligible/生成を検査する。存在すればWake又はGenerateを待ち、存在しなければmodel_failed/deadlock。T到達の共通停止を先に判定する |
| 不変条件 | 各inputのQ+reserved<=capacity、各output active<=1、routeは単調XY、hop連番連続、requestは一場所にだけ存在する。各Completeの予約解除数は一つ、local Completeは予約操作0回 |

## 4. 拡張と検証境界

| 項目 | 確定契約 |
| --- | --- |
| 拡張 | policyをprofileに静的登録し、共有バス仲裁・AHB段階・NoC経路の責務をcontext内に保持する。共有DESの不透明payload、effect batch、結果wrapperを使用し、別profile追加時は既存fixtureを回帰基準にする |

[検証仕様](../verification/cases/SoC・AHB・NoC検証仕様書.md)のDIR-TEST-0044～0049を使用する。fixture checkerは解析値・資料整合の検査であり、製品のevent実行を検証した主張は対象外（注）。
