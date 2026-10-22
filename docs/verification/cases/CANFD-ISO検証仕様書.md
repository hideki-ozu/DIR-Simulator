# CANFD-ISO検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`test-canfd-iso`
文書状態：実装ブランチ。未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。独立bit列、decoder破損、製品時間、旧profileとViewerを照合 |

<a id="dir-test-0129"></a>

```trace
{"id":"DIR-TEST-0129","stage":"verification","requirements":["DIR-REQ-0174","DIR-REQ-0175","DIR-REQ-0176","DIR-REQ-0177","DIR-REQ-0178","DIR-REQ-0179","DIR-REQ-0180","DIR-REQ-0181"],"upstream":["design-canfd-iso#codec"],"state":"confirmed","pending":[]}
```

対象は[ISO codec仕様](../../specs/models/CANFD-ISO詳細機能仕様書.md#codec)。期待bit列はDIR-Simulatorを使わず、Pierre Molinaro氏の独立ISO CAN FD実装commit `e74bb6b49885bb27f8741849553e73fefcefb11a`から生成する。[fixture出典・再現手順](../../../crates/dir-simulator/tests/fixtures/canfd_iso_wire_vectors.md)にsource hash、fixture hash、MIT帰属と生成手順を記録する。上流コードは同梱しない。

| 試験 | 独立期待値と受入 |
| --- | --- |
| codec | 全16 DLC、11/29bit ID、BRS/ESI、4payload pattern、CRC17/21、全8stuff-count codeの128frame。27,088bitすべてを上流bit列と一致させる |
| decoder | 128frameの期待内容を復元する。各frameの各bitを一つずつ反転し、期待内容を与えないdecoderがすべて拒否する。切詰め・末尾余分・不正bit値も拒否する |
| 製品実行 | 公開prepare/run経路へ128frameを与え、wire/CRC/N/DとSOF→EOFの実時間を照合する。333333/777777bpsの非整数bit期間でも全体一回のceilを確認する |
| 入力・停止 | ESI/BRS欠落、外部wire指定、Bus/global profile不一致を準備時に拒否。T=0でrequestなし、frame途中停止でEOF/receptionなし |
| Viewer | 実結果128frameを検算し、wire/CRC/ESI/count/位置/N/D改変を拒否。前後移動の再現性、partial実績、standalone HTML出力を確認する |
| 互換 | 従来precomputed profileの9件と媒体/FD Viewer試験6件が継続合格する |

2026-10-21実行：Rust `canfd_wire_codec` 5件、`canfd_iso` 3件、従来`canfd` 9件が合格。Node `canfd_iso_viewer_model` 2件、`media_fd_viewer_model` 6件が合格。外部作業証跡は`/tmp/dir-remaining-implementation-20261021-evidence/canfd-iso-{test,viewer-test}.log`。これらは理想frame符号化と製品接続の検証であり、規格全体適合や電気的実機試験の結果ではない。

```sh
cargo test --locked -p dir-simulator --test canfd_wire_codec --test canfd_iso --test canfd
DIR_SIMULATOR_BIN=target/debug/dir-simulator node --test tests/canfd_iso_viewer_model.test.cjs tests/media_fd_viewer_model.test.cjs
```
