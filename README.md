# DIR Simulator

文書ID：`readme`

**DIR = Definition（定義）、Initialization（初期化）、Runtime（実行）**

DIR Simulatorは、ネットワークおよびSoCシステムを対象としたRust製の離散イベント型シミュレータです。
次の3層モデルを採用しています。

1. **Definition（定義）** — NEDの構造記述の一部に対応
2. **Initialization（初期化）** — 必要最小限の独自INI仕様によるシナリオ設定とパラメータの上書き
3. **Runtime（実行）** — Rustネイティブのシミュレーションモジュールと実行エンジン

## 目標

- NEDの構造記述機能のうち、実用的な範囲で高い互換性を確保する
- OMNeT++のC++ APIとの互換性ではなく、RustネイティブのランタイムAPIを提供する
- 以下を共通のシミュレーションモデルで扱う
  - CAN / CAN-FD
  - Ethernet（100BASE-T1 / 100BASE-TX）
  - ゲートウェイとスイッチ
  - SoCバス / インターコネクト
  - NoC
  - DDR / SRAM
  - 共有メモリIPC
  - DMA / メールボックスIPC
- 遅延、バッファ、共有資源、調停、メトリクスを明示的にモデル化する
- 決定的な離散イベント実行を実現する

上記は評価対象とする分野です。再現する各モデルの振る舞い・精度は、[要件定義の未確定事項](docs/要件定義書.md#6-未確定事項不足の管理)で管理しています。

## 3層アーキテクチャ

![DIRの3層アーキテクチャ](docs/diagrams/readme/readme--definition-initialization-runtime--component.svg)

[図のソース](docs/diagrams/readme/readme--definition-initialization-runtime--component.puml)

### Definition（定義）

NEDでシステム構造、モジュール型、ポート / ゲート、接続、デフォルトパラメータを記述します。

### Initialization（初期化）

INIファイルでシミュレーションシナリオを記述し、インスタンスのパラメータを上書きします。

### Runtime（実行）

Rustモジュールで振る舞いを実装します。Module Registry（モジュールレジストリ）が、振る舞いを持つNEDのsimple module型とRust実装を対応付けます。compound moduleは子モジュールと接続に展開します。

ノードは必要なケイパビリティ（送受信、バッファリング、転送、調停など）を組み合わせて構成し、接続可否はポート単位で検証します。実行結果は時系列・集計値として記録し、CSVとJSONで出力する計画です。

## 互換性の方針

DIRは、OMNeT++を完全に置き換えることを**目的としていません**。

初期段階の方針は以下のとおりです。

- NEDの構造記述との互換性：対象
- `omnetpp.ini`との完全互換：対象外。DIRの設定仕様として定義
- OMNeT++のC++ランタイムAPIとの互換性：対象外
- `.msg`との互換性：対象外
- INETのバイナリ / APIとの互換性：対象外
- INET / NEDのインポートや対応付け：将来的な検討事項
- Parquet、OMNeT++の`.vec`・`.sca`出力：現時点では対象外

NEDの構文対応と、INIによるパラメータの値解決は区別します。DIRではNED型の共通のデフォルト値をINIでインスタンスごとに上書きします。対応構文とOMNeT++との意味上の差分は要件定義で管理します。

DIRは、OMNeT++の実装コードを移植するのではなく、公開ドキュメントに記載された言語の振る舞いに基づいて独立して実装する方針です。

## ドキュメント

- [要件定義書](docs/要件定義書.md)：固定ID付き要件と親子関係、分解状態、受け入れ条件、未確定事項
- [機能仕様書](docs/機能仕様書.md)：固定機能ID、入出力、正常時・境界・異常時の振る舞い、全要件から機能への対応と充足確認
- [要件トレーサビリティ一覧](docs/要件トレーサビリティ一覧.md)：要件ごとの経路を1セル1IDで横に並べた対応表と未接続項目（自動生成）。[背景色付きHTML版](docs/要件トレーサビリティ一覧.html)はローカルのブラウザで開けます。
- [アーキテクチャ設計書](docs/アーキテクチャ設計書.md)：責務分担、内部モデル、API案
- [トレーサビリティ管理規約](docs/トレーサビリティ管理規約.md)：6工程の対応記録、全経路検査、変更影響の逆引き
- [ドキュメント作成・運用規約](docs/ドキュメント作成・運用規約.md)：文書体系、要件分解・詳細化・検証の運用、記述テンプレート、文書と図の命名・配置

詳細機能仕様・詳細設計・検証仕様は、規約に定めた分野から段階的に作成します。現在は要件の初期分解と作成先を反映した段階で、詳細文書の作成・仕様確定・実装検証は今後の作業です。文書の正式名とファイル名をそろえ、自動生成レポートを除く各プロジェクト文書の冒頭に更新履歴を記載し、内容差分はGitで管理します。

図の編集元（`.puml`）と表示用SVGは `docs/diagrams/<文書ID>/` に保存しています。ローカルのPlantUMLで全図を更新・確認できます。

```bash
python3 scripts/render_diagrams.py
python3 scripts/render_diagrams.py --check
```

要件から検証までの対応は次で点検できます。通常検査は未完了を報告し、`--strict` は経路に不足があれば失敗します。詳細文書が未作成の現段階では厳格検査は未完了になります。

```bash
python3 scripts/check_traceability.py
python3 scripts/check_traceability.py --strict
python3 scripts/check_traceability.py --impact DIR-FUNC-008
```

一覧のMarkdown版とHTML版は `python3 scripts/generate_traceability.py` で同時に更新できます。`--check` で両方の生成結果が最新か確認できます。HTML版は薄い背景色で工程と未割当を区別します。Markdown表示側が装飾を除去する場合も、ブラウザでHTML版を開けば背景色を確認できます。コミット時に両方を自動更新する場合は、初回のみ `git config core.hooksPath .githooks` を実行します。

## 開発状況

初期設計 / プロジェクトの立ち上げ段階です。要件とAPIは草案であり、実行可能なシミュレータはまだ実装していません。
モデルの再現範囲、NED/INIの詳細、時間と終了条件、バッファ・調停、計測定義、実行環境などの未確定事項を要件定義にまとめています。

## ライセンス

DIR Simulatorのプロジェクト作成物は、別途ライセンス表示がある場合を除き、MIT LicenseまたはApache License 2.0のいずれか（利用者が選択）で利用できます。対象にはソースコード、文書、テンプレート、PlantUMLソース、生成図を含みます。ライセンス本文は [`LICENSE-MIT`](LICENSE-MIT) と [`LICENSE-APACHE`](LICENSE-APACHE)、適用範囲は [`LICENSE`](LICENSE) を参照してください。

第三者の素材・依存関係はこの許諾の対象外で、それぞれのライセンス条件に従います。配布条件と出自の確認方法は、要件定義書の `DIR-TBD-010` で引き続き管理します。
