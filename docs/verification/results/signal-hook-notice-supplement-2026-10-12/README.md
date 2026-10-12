# Signal-hook notice supplement (2026-10-12)

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.5`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-12` | 初版。追加signal-hook 2件のlock checksum・原文と独立補助証跡の対応を記録。旧42件監査・ライセンス経路・採用承認は維持 |

文書ID：`signal-hook-notice-supplement-2026-10-12`

This independently recorded supplement covers only `signal-hook 0.3.18` and
`signal-hook-registry 1.4.8`, newly present in the current Cargo.lock. The supplied
2026-10-08 audit ZIP, its 42-package manifest and prior results remain unchanged.
The current checker requires the exact historical 42 plus these exact two.

The official `.crate` files were read from the existing Cargo registry cache.
Their complete SHA-256 values matched the current registry checksums in
Cargo.lock before extracting any retained bytes. `supplement.json` records those
values and sizes, exact normalized/original Cargo metadata and VCS metadata,
LICENSE bytes and the README license-declaration excerpt. The checker pins the
supplement manifest SHA-256 independently so changing an evidence hash to hide
changed license bytes does not pass verification.

Only metadata, license originals and license-declaration excerpts are stored.
The archives and implementation source are not vendored here; future checks of
these stored inputs confirm pinned recorded evidence and exact notice bytes,
not a fresh archive download. Both advertised license alternatives are retained.
The original MIT copyright attribution is preserved literally. Standalone NOTICE
files and source copyright lines were not present in the checked two archives.
No license route was selected, and adoption/distribution approval remains pending.
