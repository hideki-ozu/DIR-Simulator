# AXI並列チャネル検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`test-axi-concurrent`
文書状態：基点からの追加実装。v1.2.0収録。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

<a id="dir-test-0131"></a>

```trace
{"id":"DIR-TEST-0131","stage":"verification","requirements":["DIR-REQ-0283","DIR-REQ-0284","DIR-REQ-0285"],"upstream":["design-axi-concurrent#channels"],"state":"confirmed","pending":[]}
```

正式要件はDIR-REQ-0283～0285、検証IDはDIR-TEST-0131。従来AXI要件を再定義しない。 The legacy `axi4.transaction.v1` byte-write/read expectations remain separately checked in `tests/axi.rs`.

Scope: [profile specification](../../specs/models/AXI並列チャネル詳細機能仕様書.md), [design](../../design/AXI並列チャネル詳細設計書.md), and [runnable scenario](../../../examples/transaction-fabric/scenario.ini). This verifies the declared synchronous transaction/channel model, not a clause-complete AXI4 electrical certification or the unimplemented legacy memory/IPC bridge.

| Production test | Independent expected result |
| --- | --- |
| `independent_channels_cross_id_out_of_order_same_id_order_and_shared_dma_visibility` | Fast R precedes slow R across IDs; same-ID response follows both slow beats. W0 precedes AW3; commits begin at3; B follows the last commit. Direct DMA sees exactly AXI-written bytes20–27. Disjoint paths accept two AR at0. |
| `wrap_fixed_narrow_unaligned_and_byte_strobes_preserve_other_lanes` | WRAP addresses12,0,4,8 and memory beat values22,33,44,11. FIXED's last write wins at the repeated address. Width8/size4/start3 transfers lane3 first, lanes4–7 next and lanes0–3 last; disabled byte5 retains initial6. |
| `exclusives_invalidated_by_dma_and_successful_exclusive_is_visible` | DMA interference makes the first exclusive W perform no writes and return OKAY; new read followed by uncontended exclusive W returns EXOKAY and installs99. |
| `decoder_errors_and_invalid_boundary_strobes_routes_are_rejected` | Unmapped address returns DECERR. 4 KiB crossing, out-of-range WSTRB, unaligned WRAP, overlapping targets and unknown route nodes reject preparation. |
| `midburst_checkpoint_restores_buffered_channels_and_byte_memory` | Save/restore after2/5/10 public session steps, including W buffered before AW, partial memory writes and a response held under backpressure. Resumed actual records and memory snapshots equal uninterrupted execution exactly. |
| `horizon_observes_real_memory_commits_without_inventing_b_response` | Horizon2 retains exactly two real write beats, updates exactly bytes0–7, and has no B. Horizon0 has no channel record. |
| `shared_memory_write_preflight_has_no_partial_mutation` | A write whose enabled range crosses a region boundary fails without changing its earlier valid bytes. |
| `finite_w_buffer_response_payload_holds_under_backpressure_and_b_can_reorder` | Capacity1 stalls W between tick0 and5 until AW4 permits a commit; next transaction W waits until prior WLAST. R VALID1/READY6 keeps initial bytes despite write3 and blocks a competing same-manager R. Independent-ID B can overtake another target's delayed B. |
| `exclusive_read_grant_waits_for_last_r_and_intervening_write_cancels_candidate` | DMA between exclusive AR and stalled RLAST prevents subsequent exclusive W from writing even after the read completes. |

Executed focused command (with Rust1.85.0 repository toolchain):

```sh
CARGO_TARGET_DIR=/tmp/dir-remaining-implementation-target /home/hideki/.cargo/bin/cargo test --locked -p dir-simulator --test transaction_fabric --test axi
```

Result: 9 concurrent-fabric tests and 11 legacy-AXI tests passed. Existing profile behavior is independently exercised by the legacy fixture suite; no profile-count assertion was changed.

Public CLI sample was executed from the actual built binary with output at `/tmp/dir-remaining-implementation-20261021-evidence/transaction-fabric-final-run` and standalone Viewer at `/tmp/dir-remaining-implementation-20261021-evidence/transaction-fabric-final-viewer.html`. Observed sample: fast R2 before slow R8/9; W0/1 precedes AW3; byte commits3/4, B5, and direct DMA reads the committed bytes at11/12. It ends normally with13 committed callbacks and17 model records. The existing Node `controlled-model.js` parser accepted the actual schema3 result and the product `view` command produced a self-contained HTML artifact. This is parser/publication verification; no interactive browser screenshot or hardware/RTL reference run was performed.

The focused `cargo clippy --locked -p dir-simulator --lib --test transaction_fabric -- -D warnings` gate passed after the concurrent CAN/network diagnostics were resolved. Family source/test files also passed `rustfmt --check`. The central owner runs repository-wide validation after integrating all workstreams. Formal requirement traceability, release claims, full standards conformance, complete M01 integration and performance limits remain separate evidence scopes.
