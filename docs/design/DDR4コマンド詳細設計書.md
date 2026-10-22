# DDR4コマンド詳細設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`design-ddr4`
文書状態：追加実装・v1.2.0収録。規格全体適合と実行検証の範囲を区別する。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="commands"></a>

```trace
{"id":"design-ddr4#commands","stage":"design","requirements":["DIR-REQ-0299"],"upstream":["architecture#ddr4"],"state":"confirmed","pending":[]}
```

`types.rs`が固定mode/hashとtyped要求・command・byte effect、`device.rs`がdigital command state、`controller.rs`が有限queueと合法command仲裁を担当する。MemoryServiceとのinterfaceはbyte effectだけで、private RAMを作らない。SoCが全subserviceの次deadlineの最小値でglobal schedulerを進める。

Deviceは16bankのrow、ACT/PRE/read/write履歴、bank group別の最新write、rank/MRS/nonMRS/DLL/ZQ/SRX deadline、DQ予約、refresh debt/history、訓練状態と物理effect待ちを所有する。同時刻順序は予約済みphysical effect→burst完了→要求完了→次command仲裁とする。一つのrising edgeで一つのcommand。データはhalf cycleごとに実byteとして返す。

Serviceは年齢順に要求を走査し、古い未完了要求と物理範囲が重複する要求は追い越させない。独立bankには別要求のACT等を進める。refresh期限ではPREAとREFを優先する。合法minimum待ちの拒否は最短deadlineを返し、不正state/encodingと区別する。Device issue/advanceとService advanceはcandidateを検査してから置換し、失敗時に予約やbyte effectを部分公開しない。

checkpointは所有stateをserializeし、`Service::validate`が要求からburst形状・残存byte effect・DQ intervalを再構成して厳密照合する。Deviceはmode/MR/訓練・時刻・bank/history・pending参照を検査する。初期化完了は該当phaseだけ一つ、先行初期化commandはstateから再構成した必須deadlineを持つ。global FES側のtimerとこの次deadlineの対応も統合codecで検証する。実共有byte、validity、cache/leaseとtargetの所有関係はSoC codecの検証対象である。

[検証仕様](../verification/cases/DDR4コマンド検証仕様書.md)では独立literal期待値、別構造のcommand checker、実production targetの完了時刻とbyteを別々に確認する。
