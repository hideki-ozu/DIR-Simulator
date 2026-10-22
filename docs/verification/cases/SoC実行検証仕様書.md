# SoC実行検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
公開状態：v1.2.0収録。実装台帳に記録した契約・検証範囲を参照。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

文書ID：`verification-soc-execution`
文書状態：実装対応草案。受け入れ契約と実行済み証跡を区別する。

<a id="dir-test-0137"></a>

```trace
{"id":"DIR-TEST-0137","stage":"verification","requirements":["DIR-REQ-0295","DIR-REQ-0296","DIR-REQ-0297","DIR-REQ-0298"],"upstream":["design-soc-execution#execution"],"state":"confirmed","pending":[]}
```

## 受け入れ群

正本は`crates/dir-simulator/tests/soc_execution.rs`、独立CPU oracleは`tests/rv32i_oracle.rs`。下表は受け入れ契約であり、実行結果・未通過項目は作業証跡へ別途保存する。

| 群 | 期待値 |
| --- | --- |
| CPU | Spikeの独立期待値と全base命令形式・seeded operands・trap/PC wrapを比較し、実programのfetch/load/storeを実transportで実行 |
| coherence | 2core handoff、conflicting write、dirty intervention、eviction、MSHR=1、FENCE drain、coherent DMAの実byteを比較 |
| native composition | AXI/AHB/NoCをそれぞれ単独に同じlogical read/writeへ適用し、enabled byteとerror・完了境界が一致。native AW/W順序・AHB address/data/HREADY・実NoC flitを観測 |
| lease | chunk境界horizonで未完成slotをReadyにせず、実write完了後publish、generation再利用、早期/stale/重複操作を拒否。重複physical slot inputを拒否 |
| DDR | 実halfcycle byteeffect以前にmutation/responseしない。SRAMからのpopulate、dirty writeback、三adapter、未定義read/reset、init中checkpointを確認し、event数で初期化pollingを検出 |
| checkpoint | 複数transient境界の全record・byte・CPU最終状態が無中断と一致。rehashed blobでもfetch/DMA/ACK/owner/native/NoC capture/DDR/timer tamperをpanicなくload拒否 |
| 製品 | examples/soc-executionをproduction CLI validate/run/viewしschema3 Viewerを確認。通常gateは親統合で実行 |

focused gateは`cargo test -p dir-simulator --test soc_execution`および`--test rv32i_oracle`。SRAMの試験成功だけでDDRや全SoC/AMBA規格適合を完了扱いしない。

## mapped mailboxの追加検証

`mapped_mailbox_*`、`cpu_fence_and_dma_register_access_use_uncached_mailbox_target`、`mailbox_semantic_history_*`、`invalid_register_dma_descriptor_cannot_partially_stage_a_command`、`ddr_only_reset_does_not_abort_pending_sram_mailbox_native_target`を追加する。

三transportで同じgeneration・payload・通知とlatencyを比較し、部分/zero strobe・未整列・幅・register跨ぎ・atomic・異source・不完全publishを検査する。CPUのFENCEとuncachedアクセス、DMAの4byte register descriptor、同一ID atomicと先行endpoint待ちの終了を確認する。複数checkpoint境界で全recordの継続一致を比較する。

semantic mutantはmanifest/codecref/blob SHAを正しく付け直したbundleを用い、無変更のrehashed bundleがloadできることを先に確認する。slotのReady偽装、length=0でのReady偽装、通知削除、origin削除、register read・native capture・logical resultの同時改変を、モデル固有の意味検証で拒否させる。ハッシュ不一致だけを根拠に成功と数えない。
