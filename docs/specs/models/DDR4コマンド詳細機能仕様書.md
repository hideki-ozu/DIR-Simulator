# DDR4コマンド詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`spec-ddr4`
文書状態：追加実装・v1.2.0収録。規格全体適合と実行検証の範囲を区別する。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="commands"></a>

```trace
{"id":"spec-ddr4#commands","stage":"spec","requirements":["DIR-REQ-0299"],"upstream":["DIR-FUNC-0081"],"state":"confirmed","pending":[]}
```

## 対象と版

ISSI IS43QR85120B-083RBL、4Gb x8、Rev FのDDR4-2400R 16-16-16を固定する。デバイス資料の著者はISSIであり、取得物は[配布mirror](https://www.uttc.com.tw/wp-content/uploads/2026/01/43-46QR85120B-16256B.pdf)の274 page PDF、SHA-256は`0606a9501dc13de5934856c9d7fbe0310f6a4edacf26b9cf416f4a5bfc2ca727`。表紙と一部footerの日付は05/20/2024、05/24/2024が混在する。[77規則台帳](../../implementation/ddr4-rules.json)で出典page・対象・正負境界を固定する。

mode IDは`issi.is43qr85120b-083rbl.rev-f.ddr4-2400r.x8.bl8.1n.25c.v1`。tCK 834ps、half cycle 417ps、CL16、CWL12、AL0、PL0、BL8 sequential、bank16・bank group4、25℃・1x refreshである。定常clockは常に稼働し、jitterのない理想digital pin契約とする。clock停止や周波数・温度・preamble/electrical option変更は、このmodeへ暗黙変換しない。DDR電気波形・signal integrity・実機訓練marginの精度は主張しない。

## byte ownerと非同期完了

`runtime::ddr4::Service`が有限論理queue、`Device`がbank/rank・MR・power/training stateを持つ。どちらもRAM配列を持たない。`enqueue(Request)`は一意の活動ID、領域内1～65536 byte、書込みなら同数のbyte enableを要求する。queue容量は1～1024、regionは8 byte整列、8の倍数で最大512MiB。address mapはcolumn=offset[9:0]、flatten bank=offset[13:10]、row=offset[28:14]とする。

論理要求は整列した8 byte BL8へ分割し、端の無効byteをDMでmaskする。`advance(at_ps)`は時系列の`ReadByte`/`WriteByte`と`BurstComplete`/`RequestComplete`を返す。共通のMemoryServiceを持つ呼出側がその実時刻でbyteを取得・更新する。maskされたwrite beatは更新せず、queue受理・ACT・WR commandだけでbyteを書かない。transportのR/B/HREADY、cache fill、lease publishは必要な実転送完了まで進めない。固定応答delayで実DDR処理を置き換えない。

## commandと継続

実ACT、PRE/PREA、RD/RDA、WR/WRA、REF、MRS、ZQCL/ZQCSとRESET/CKE、self-refresh、Vref/write-levelingを検査する。bank rowとrank busy、bank group turnaround、tFAW、DQ占有を分ける。別bank groupの操作を挟んでも過去の適用可能なtWTR_Lを保持する。auto-prechargeと外部PREのpower-down制約を区別する。

初回はRESET low 200us、release後500us、CKE後324cycleを経てMR3/6/5/4/2/1/0、DLL reset、ZQ初回1024cycleを実行する。MRS間8cycleと非MRS24cycleを分ける。REFは全bank idleと実DQ完了、tRFC312cycleを要求し、controllerはrefreshを優先する。初期化・idle refreshの待機は次の実deadlineへ移動する。

Vref entryのcode bitsはdon't-careとして直前の実設定値を保持する。entry settling後の許可されたdummy trafficを認め、最初の値設定と後続single-step/multi-stepを区別する。WLEVのQoffはfeedbackを抑止し、出力停止やexitで未配送feedbackを取消す。WLEV exit後も次のMRSは8cycle、通常commandは24cycleを分ける。

RESETは破壊的で、未完了beatと格納byteのvalidityを無効化する責務を呼出側と共有する。self-refreshは格納byteを保持する。実byte保持・無効化の製品検証は統合targetで確認する。型定義と具体的なAPIは[types](../../../crates/dir-simulator/src/runtime/ddr4/types.rs)、[controller](../../../crates/dir-simulator/src/runtime/ddr4/controller.rs)を正本とする。

[設計](../../design/DDR4コマンド詳細設計書.md)、[検証](../../verification/cases/DDR4コマンド検証仕様書.md)。
