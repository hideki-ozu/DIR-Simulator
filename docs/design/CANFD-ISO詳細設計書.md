# CANFD-ISO詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-canfd-iso`
文書状態：実装ブランチ。未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。pure codecと準備・実行・出力・Viewerの接続を記録 |

<a id="codec"></a>

```trace
{"id":"design-canfd-iso#codec","stage":"design","requirements":["DIR-REQ-0174","DIR-REQ-0175","DIR-REQ-0176","DIR-REQ-0177","DIR-REQ-0178","DIR-REQ-0179","DIR-REQ-0180","DIR-REQ-0181"],"upstream":["architecture#canfd-iso"],"state":"confirmed","pending":[]}
```

規範契約は[詳細仕様](../specs/models/CANFD-ISO詳細機能仕様書.md#codec)。`runtime/canfd/codec.rs`は所有する`Frame`から`WireFrame`を返すpure関数を提供する。`rate_phase`はNominal/Data/Brs/CrcDelimiterを区別する。後二者は一つの速度で近似しない。`phase_bits`はframe全体の有効N/D、`duration_ps`はchecked整数演算による一回のceilを返す。

decoderは先頭からdynamic stuffを除いてheader/DLCを読み、確定payload長の直後でdynamic区間を終了する。固定stuffを検証しながらcount/parity/CRCを取り出し、受信したdynamic区間を使ってCRCを検算する。期待内容との比較だけに依存しない。`verify(expected,bits)`は別に完全一致検証を提供する。

`input/canfd.rs`はprofileごとにframeキーを限定し、ISO時だけcodecを呼ぶ。生成wireの位相長・仲裁bit・CRC/stuff位置を不変cacheへ保持する。専用FD engineはその位相長を実送信時間へ使用する。外部入力位相長への置換は行わない。従来のprecomputed cacheにはISO metadataを付けない。

`output/canfd.rs`はISO frameをversion 2、request/receptionをversion 1として投影する。`output/preparation.rs`とdefault Registryのcatalogもこの組合せを宣言する。Viewer `model.js`は別実装の整数bit/CRC処理でwire recordを検算し、既存の実績milestone replayへ接続する。`app.js`は選択frameのwire情報を表示する。

有限queue・仲裁・受信filter・時刻overflowの失敗prefixは既存FD engineが所有する。codecは未来の実績を作成しない。エラー状態・混在bus・再送はこの実装から独立したprotocol機械の責務である。
