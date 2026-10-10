# 通常VSCodeへの実行依頼

文書バージョン：`1.1.0`  
文書ID：`ac0008-vscode-handoff`  
状態：実行依頼案。未送信。

### 更新履歴

| 文書版 | 日付 | 内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-10 | PR #42第2pushで初回保存 |

通常のVSCode/WSL Ubuntu24.04で、既存checkout・未コミット変更を保持した独立作業領域からPR #42を検証してください。PR #40をbaseとする依存と、取得した正確なhead/source commitを記録してください。

Rust1.85.0で新規 `ac0008_paths` と `ac0008_ned_rejections` のcompile/fmt/testを行い、コマンド・環境・exit code・ログ・source/test commitを保存してください。例：`cargo test --locked -p dir-simulator --test ac0008_paths --test ac0008_ned_rejections --no-run`、`cargo fmt --all -- --check`、同じfocused test指定で実行。失敗は既存作業の失敗と混同せず、このheadの結果として記録してください。

Issue #41の要件を変更せず、display/descriptionのUnicode/escape値、宣言/parameter所有者、元位置、BOM/CRLF、同型複数instanceからの参照を保持する限定修正と回帰検証をお願いします。仕様緩和やClassical CANの汎用factory移行は行わず、実際の診断/所有者assertionをatomic case IDへ対応付けてください。

既存 `ac0008-status.json`、`fresh-candidate-reverification.json` と、対応するbuild/link証跡（rustc/Cargo/host/target/features/flags、build graph、ELF interpreter・dynamic/static dependencies、system library版、Python wheel/Playwrightの人口区別・hash）を提供してください。既存の569/29集計を個別合格へ置き換えないでください。

完成済み固定artifact3705 member・6manifest・42crateの全件監査/再取得/Release再構築は依頼範囲外です。merge/tag/release/deploy、権限・設定変更、外部依頼は行わないでください。
