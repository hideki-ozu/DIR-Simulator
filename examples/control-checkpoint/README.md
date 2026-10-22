# Cooperative control, integer metrics and persistent checkpoint

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`example-control-checkpoint`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | v1.2.0の対象タグ・文書版を確定。過去の測定基準と公開済み履歴を保持 |
| `1.1.0` | `2026-10-17` | 協調制御・整数指標・同一binaryのgeneric checkpointを別processへ復元する例の実行手順をv1.1.6へ収録 |

Run from the repository root with the pinned Rust toolchain. Use fresh output directories.

```sh
cargo run --locked -p dir-simulator --example control_checkpoint -- run examples/control-checkpoint/scenario.ini /tmp/dir-control-baseline
cargo run --locked -p dir-simulator --example control_checkpoint -- save examples/control-checkpoint/scenario.ini /tmp/dir-control-saved
cargo run --locked -p dir-simulator --example control_checkpoint -- restore /tmp/dir-control-saved
```

The `save` process pauses after the first 7ps event, leaving another 7ps event pending. It exports a non-destructive schema-3 result and writes a real checkpoint directory. `restore` runs in a new process without calling model initialization again. Its final `simulation` values match the uninterrupted run; session/export metadata differs.

The example registers an explicit model codec, saves RNG state and both live and retired timer references, and keeps exact gauge and point metric accumulators. The gauge starts at 2, changes to 4 at 3ps and to 9 then 1 at 7ps. Its area is 22 at the pause and 25 at the 10ps observation horizon; the final mean is 2.5. Width-4ps windows have areas 10, 13 and 2. The point minimum at the 7ps pause is 1 globally, while the 7ps point is excluded from half-open windows.

A checkpoint requires exactly the same executable bytes and registry/codec versions. Rebuilding after source changes intentionally prevents restoring an older checkpoint. `Pause`/`Stop` do not call cleanup; explicit close, natural completion and Drop handle cleanup once.

The modes are positional arguments to the example, not normal CLI flags: `save CONFIG OUTPUT_ROOT`, `run CONFIG OUTPUT_ROOT`, and `restore OUTPUT_ROOT`. Outputs are `completed/results.json`, `paused/results.json`, `checkpoint/`, and `restored/results.json` beneath the selected root. For guaranteed executable identity, build once with `cargo build --locked -p dir-simulator --example control_checkpoint` and invoke `target/debug/examples/control_checkpoint` for all three processes without rebuilding. Network checkpoint remains unsupported.

Current APIs, output and Viewer boundaries are documented in [the implementation guide](../../docs/guide/reference/後続10分野の現行実装と利用境界.md). Final validation scope is owned by [the product report](../../docs/verification/results/extension-product-2026-10-17.md); these examples do not claim full standards conformance.
