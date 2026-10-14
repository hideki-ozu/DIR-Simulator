# Ethernet TAS送信窓の実測検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-tas`
文書状態：実施範囲と残る制約を記録

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-14` | 初版：生成時刻1ps境界の正式CLI実測、隔離ZIP、guard/wake、実Viewerとサイト・負例・追跡入力検証を追加 |

## 正式sourceと測定

WSL Ubuntu24.04 LTS x86_64、Rust1.85.0、正式source `b45515644dfc65c205216d564d5bf7ef00280622`。新規正式clone・SHA一致guard・locked release build・PATH解決・公開time128000.ini validateを実行した。全0の誤SHAはerrexitなしでも終了1、代替cargo関数未呼出。新buildのbinary SHAは測定CLIのSHAと一致。

[`measurements.json`](measurements.json)にfresh全3simulation（model_records、records、summary）とbinary/source/lock/input SHAを保存する。共通1フレームへ整理した後、generators[0].times_ps[0]だけを127999/128000/128001psへ変更。old/fresh各18操作、各条件でrepository/隔離ZIPの全simulation一致。公開8memberのbyteを照合。

127999/128000psは即SOF、EOF703999/704000、release799999/800000ps。128001psはSOF2000000、EOF2576000、release2672000psで、queue待ち1871999、配送遅延2447999ps。すべて1frame/copy/受信、drop/未完了0。guard_blocked2 recordのnext_wakeは2000000、初期gateのnext_boundaryは800000ps。guard_waitは時間でなく判定件数で、guard記録がない前2条件ではsummary行も出力されない。

## 検証と保存物

- `verify_guide_ethernet_tas.py`：一変数の厳密比較、元model/NED byte同一、各CLI正常終了、manifest byte/hash、clean正式source、完全終了、CRC/64byte、各時刻・guardの等号・wake/境界の区別・CBSなし/PSFP bypass、flow4成分とbit量、隔離ZIP全simulation一致。
- `verify_ethernet_tas_single_variable_regression.py`：余分field・2フレーム化・誤時刻・INI終了時刻変更の4負例をCLIアクセス前に拒否。
- [`viewer-browser.json`](viewer-browser.json)：実Chromiumで500000psの送信中/待機counter・1行・全実行JSON、open P7、busy/guard_blocked、次境界/次wakeを検査。指定fresh9入力hashを保存し、実390px gateカード3 PNGを撮影。全3画像の実画素を確認。
- [`site-browser.json`](site-browser.json)：23ページの画像・390px横overflow、日本語「送信」「待ち」の新記事到達、pageerror/失敗/外部request0を検査。desktop/top390/測定390の画素も確認。
- 両browserの省略/不存在/指定Viewer欠落の6負例をold valid残存時に拒否し、対象を復元。実metadataパーサー・両生成一覧、commit併記の負例・復元、navとHTML集合も確認。
- MkDocs strict、24 HTMLのローカルリンク/画像/アンカー、全16 ZIP、文書Pythonテスト、全体と適用REQ0248〜0251 strict traceability、生成鮮度を検査。push前のGit indexとcommit/clean checkoutで8入力が実際に追跡されることも確認する。

共通README/入口/統合手順のpush単位改訂と全過去履歴、既存20記事・画像・15 ZIPを保持する。製品コード・workflowは変更しない。remote exact head CIはpush後に確認し、PR本文へ実結果を記録する。

初回サイトcheckerは複合語「送信窓」の検索結果待ちでtimeoutした。実診断で「送信窓」「待ち時間」「TAS」はNo results found、「送信」「待ち」では新記事に到達することを確認した。既存検索設定を変更せず、合格記録は後者2語の範囲に限定する。前者を検索成功として扱わない。

## 制約

単一frame、直接接続、空payload、P7、理想共有clock、固定GCL、伝搬/処理0のみ。CBS、PSFP filter、GCL更新、他class、満杯、複数hop、停止途中、連続負荷、実機・規格全体適合は未検証。Viewer詳細は全実行recordであり、500000psに未来のSOF/EOFを観測済みと扱わない。
