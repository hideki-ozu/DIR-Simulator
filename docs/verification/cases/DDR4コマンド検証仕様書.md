# DDR4コマンド検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-ddr4`
文書状態：追加実装・v1.2.0収録。規格全体適合と実行検証の範囲を区別する。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="dir-test-0138"></a>

```trace
{"id":"DIR-TEST-0138","stage":"verification","requirements":["DIR-REQ-0299"],"upstream":["design-ddr4#commands"],"state":"confirmed","pending":[]}
```

`DIR-TEST-0138`は[77規則台帳](../../implementation/ddr4-rules.json)の各positive/negativeを対象とする。minimumは境界と1cycle早期、maximumは境界と1cycleまたは1ps遅延を使う。runtimeのeligibility helperを期待値へ流用しない。理想continuous clockにより固定されるclock hold/setupはconfiguration不変条件として検証する。

検証コマンド：`CARGO_TARGET_DIR=/tmp/dir-remaining-implementation-target /home/hideki/.cargo/bin/cargo test -p dir-simulator --test ddr4`。

[製品テスト](../../../crates/dir-simulator/tests/ddr4.rs)で初期化command列、ACT/bank group/tFAW、CAS方向転換、PRE/AP、実DM/halfcycle、refresh/tRFC、MRS、PD/SR、Vref/WLEV、queue上限、保存継続と意味破損を確認する。別bank groupを挟んだwrite→read、WRA35cycleと外部PRE2cycleの区別、Qoff/exit以前の未配送feedback、必須初期化deadlineの消失/複製を回帰対象に含める。

現時点の製品テストは27件、内部状態に直接到達する独立guardは2件、独立Python monitorは2件が成功している。monitorは実controller traceの初期化・open-page読書き・refreshを検査し、早期command、欠落/改変byte、早期完了、別bank groupを挟むwrite履歴、末尾refresh欠落の7種類の変異を個別理由で拒否する。power/training/APの検証はRustの境界試験と区別する。

[規則別の検証対応表](../../../crates/dir-simulator/tests/fixtures/ddr4_oracle/coverage.json)は77行中、独立monitorの直接検査33、別のRust境界試験40、部分検査0、configuration不変条件4、未検査0としている。RDA→ACTとWRA→ACTは内部PRE+tRPの合成式を1cycle早期と境界で検査する。power-down最短保持はCKE pulseと同じ6CKであり、5CK拒否/6CK成功が両制約の交差を検査する。別名guardの単独発火と独立Python coverageは主張せず、checker順序も変更しない。理想continuous clockに固定した4行は外部clock停止を実行したという意味ではない。対応表の分類は実装の有無、独立性、規格全体への適合を同一視しない。

実MemoryServiceを唯一のbyte所有者として、非同期AXI/AHB/NoC、CPU/cache、DMAとslot leaseへ接続している。RESETは対象DDR範囲の要求とbyteを破壊し、SRは実byteを保持する。SoCの別process checkpoint継続ではnative要求、DDR予約、global timer、要求owner/epoch/chunkを照合する。CLIとViewerによる実行結果も別に保持する。これらは選定したISSIデバイス、固定clock、実装したcommand subsetの証拠であり、JEDEC全体の認証結果ではない。

`queued_independent_banks_overlap_but_share_ca_and_dq_buses`は、二つの要求を同時enqueueした別bank groupの実完了が直列enqueueより早いこと、CAは1CK当たり一command、CAS間隔4CK、DQは半cycle当たり一byteを守ることを確認する。二つ目のCASは一つ目の要求完了を待たずに進む。
