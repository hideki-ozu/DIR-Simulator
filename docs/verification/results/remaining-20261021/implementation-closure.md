# 追加81項目の実装・検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`verification-remaining-implementation-closure`
文書状態：未commit・未公開。最終判定は実装台帳と検証結果JSONを参照。

未実装調査から抽出した[81項目](../../../implementation/remaining-capabilities.json)を対象とする。CPU/cache/memoryは合意したRV32I・MESI・DDR4、Ethernetは仕様で選択した版・mode・数値精度の契約を基準にした。

| 対象 | 項目数 | 製品経路と確認の入口 |
| --- | ---: | --- |
| CAN / CAN FD | 17 | 実bit列、誤り・再送・bus-off、Gateway、混在bus。各行のfixtureと独立wire照合 |
| Ethernet PHY / PLCA | 9 | 実sample、codec/FEC、startup/AN、fault回復、共有medium、repeater、選択PLCA Control/Data |
| clock / TSN | 6 | 実gPTP packet、時計補正とTAS/CBS/PSFP、Qcc、half-duplex / PAUSEの型付き適合条件 |
| bridge / network合成 | 13 | FRER、MAC Merge、IGMP/MLD、RSTP/MSTP、provider、Gateway複合とserializer/reset |
| bus / NoC / SoC / memory | 27 | AXI/AHB実チャネル、NoC flit/VC、RV32I、MESI、MMIO/DMA/mailbox、DDR4 command規則 |
| zero horizon / 制御・保存・結果 | 9 | 初期・成功prefix、13 legacy profile、checkpoint版移行、Metric V2、Viewer巻戻し |

## 最終ゲート

| 検証 | 結果 |
| --- | --- |
| Rust workspace | 95対象、1820 PASS・0 FAIL・59 ignored。標準の`cargo test --locked --workspace` |
| Rust整形・Clippy | `cargo fmt --all -- --check`、全targetを`-D warnings`でPASS |
| 実sample gPTP | 0/16/17sample伝搬境界でstartup・実delimiter captureを照合しPASS（254.04s） |
| 制御legacy / Viewer model | 14profile×3段階を最終libraryで出力。現CLIとViewerで142 PASS・1任意fixture skip |
| standalone HTML | PAUSE、configured gPTP、abstract reset、PLCA pending、Gateway FRER、raw gPTPの6件。PC/mobile、巻戻し、browser error/external requestなし |
| Python / guide / 図 | 67試験、MkDocs strict、PlantUML13件PASS |
| 文書追跡 | 296要件・91機能・414node、構造error/不足0。3生成物を候補indexから生成し実indexを保持 |

最終compiled source SHA256は`549d386cc0a15e99fc0c3e9073d7956d2935aa462289be62d4b067be3a08ac8c`。CLIの実出力と14profileのmetadataにも同値を確認した。build並列数は2、試験threadは8、AF_UNIX制約を満たす短い専用TMPDIRを使う。最初の無制限並列linkはメモリ不足で試験開始前に停止したため、logを保持して同source/profileで再実行した。

## 回帰で修正した境界

- 入力INIの二重取得を、取得済みparse結果とsnapshotの受け渡しへ修正した。元の参照情報試験を変更せず、ordinary/reference/registry経路と診断・provenanceを確認した。
- configured MAC MergeのNative保存照合で、送信PHY・伝搬・受信PHYの全遅延を既存helperから計算する。整合する+1ps改変も拒否し、失敗時にlive backend/FESを変更しない。
- source down/up後に同じ将来時刻へ予約し直す場合、古いwakeは無効、新しいwakeは有効で、共通Control FESを経て実SOFになる。
- LLDPを通常のport classで扱うため、Gateway/Qcc例はbootstrap用classを明示した。データのPCP/class、元の時刻・帯域計算・両方向の検証条件は維持した。
- checkpoint試験の古い数値indexをfixture pathに置き換え、実行中・代表状態・別process復元・canonical一致の条件を保った。
- PLCAのforced MAC JAMと実physical collisionを区別し、pending timer、既存TX oscillatorに同期するMCD/PMCD、遅延nibbleのFLUSHと制御groupの排出を確認した。

元の失敗logと修正後のlogを別に保持する。規格本文PDF/textはローカル`tmp/standards/`を参照し、配布物へ含めない。source manifestは参照版・hashと対象の実装・試験hashを記録する。

## 作業場所と公開状態

実装は独立worktree `/tmp/dir-remaining-implementation-20261021` にある。元の `/home/hideki/DIR-Simulator` はHEAD、変更状態、元からある4ファイルのhashを照合して保持した。commit・push・Qiita投稿は行っていない。

`verified`は各行の宣言した契約に対する製品経路と検証の判定である。未知のcomponentの合成、宣言外のPHY option、hardware電気認証、実行していない性能条件へ判定を拡張しない。

| 版 | 日付 | 変更 |
| --- | --- | --- |
| 1.1.0 | 2026-10-22 | 合意した81項目の実装・検証と再現条件を記録。未公開 |
