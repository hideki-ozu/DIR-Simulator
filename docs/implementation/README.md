# 追加実装と検証の対応

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`implementation-remaining-capabilities-index`
文書状態：v1.2.0収録。

本書はv1.2.0の現行入口です。[リリース検証記録](../verification/results/release-v1.2.0-2026-10-22.md)に文書採番、実装commitからの差分、公開用buildとGitHub検査の確認方法を記録します。以下の2026-10-21開始の検証snapshotにある「未commit・未公開」とsource hashは、当時の実行状態を保持しています。

未実装調査で抽出した81項目の契約、判定と証拠は[実装台帳](remaining-capabilities.json)で管理する。`verified`は各行で指定した契約の製品経路を確認した状態であり、実行していない範囲の判定には使わない。全体の状態は同JSONの`state`を参照する。

| 分野 | 実装の基準・入口 |
| --- | --- |
| CAN / CAN FD | 実bit列、誤り状態・再送、選択したISO FD契約。既存の理想通信profileも保持する |
| CPU / cache / SoC | 合意したRISC-V RV32I、MESI、実MMIO・DMA・mailbox。`examples/soc-execution/` |
| memory / bus / NoC | DDR4の版・timing規則、AXI/AHB実チャネル、NoC flit。各profileの仕様に対応するfixtureで確認する |
| Ethernet / PHY | 選択した802.3-2022の1000BASE-T1、10BASE-T、10BASE-T1S/PLCAと、宣言した数値channel。規格の参照先・hashは[PHY source manifest](../verification/fixtures/ieee-phy/source-manifest.json) |
| TSN / bridge | gPTP実packet、TAS/CBS/PSFP、FRER、Qcc、RSTP/MSTP、provider/shared domainsとMAC Control。合成時は型付きadapterのowner/duplex/clock契約を検査する |
| 結果・保存・表示 | checkpointのsource照合と版移行、Metric V2、実結果のViewer巻戻し。予定時刻と実績を区別する |

仕様の入口は[機能仕様書](../機能仕様書.md)、合成条件は[型付きネットワーク構成](../specs/models/型付きネットワーク構成詳細機能仕様書.md)、個別の合否は[製品検証記録](../verification/results/remaining-20261021/)を参照する。標準本文PDFはローカルの`tmp/standards/`に置き、Gitの配布物には含めない。

| 版 | 日付 | 変更 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |
