# 受け入れ検証実施記録（2026-10-08）

文書ID：`acceptance-results-2026-10-08`

検証を実施した。機能回帰・全入力のCLI/出力・ブラウザ検査の結果と、受け入れ条件全体の承認を区別する。全体の受け入れ完了には至っていない。

[機械可読の総合記録](acceptance-2026-10-08.json)にバイナリ・入力・ソースhash、コマンド、個別結果、環境と残項目を保存した。未commitの開発中ソースの証跡で、公開タグやnative基準機の証跡ではない。

## 実行結果

| 検査 | 結果 | 証跡 |
| --- | --- | --- |
| Rust全体回帰 | 521件合格。試験強化を含む最終ソースで全体実行 | [gates](acceptance-2026-10-08/gates.json)、[追加照合](acceptance-2026-10-08/core-projections.json)、[37指標](acceptance-2026-10-08/output-acceptance.json) |
| JavaScript・NED実ブラウザ | 165/165件合格、skip 0 | [log](acceptance-2026-10-08/javascript.log) |
| 利用手順・条件差・打切り・反復 | 33項目/49コマンド合格 | [workflow](acceptance-2026-10-08/workflow.json) |
| 全INI入力のvalidate/run/view・manifest | 271入力、passed。準備失敗38件と実行失敗1件は規定の負例 | [catalog](acceptance-2026-10-08/catalog.json) |
| 新3モデルのViewer | production assets 119/119、CLI生成HTML 119/119を実ブラウザで検査 | [catalog](acceptance-2026-10-08/catalog.json) |
| 独立fixture・製品出力の解析値照合 | fixtureだけの計算と実製品projectionを分離して記録 | [checkers](acceptance-2026-10-08/fixture-checkers.json) |
| Python文書検証 | 39件合格 | [log](acceptance-2026-10-08/python.log) |
| fmt・clippy・release build | 合格 | [gates](acceptance-2026-10-08/gates.json) |
| strict traceability | 構造エラー・不完全項目0 | [log](acceptance-2026-10-08/traceability.log) |

実行時の全件合格は、検証仕様の全チェック行の合格と同義ではない。条件別の完全対応を確認できたものだけを合格とし、追加した故障注入や独立期待値への照合の範囲を以下の表とJSONに残した。部分確認は未実装や製品不良を意味するものではない。

## 100万要求の実測

32送信元×31,250要求、通常・高負荷・過負荷の3条件を、各ウォームアップ1回＋測定3回、同時実行1件で測定した。CLI起動からmanifest確定・終了までを対象とし、wall目標120秒、RSS目標2GiBで判定する。観測上限は180秒。

| 負荷 | 完了/試行 | 測定3回の停止までのwall秒 | 測定3回の観測RSS MiB | 結果 |
| --- | --- | --- | --- | --- |
| rho-0.30 | 0/4 | 180.1, 180.16, 180.24 | 116.004, 116.211, 115.441 | failed |
| rho-0.90 | 0/4 | 180.19, 180.09, 180.07 | 115.887, 116.047, 116.355 | failed |
| rho-1.20 | 0/4 | 180.13, 180.21, 180.15 | 115.234, 115.195, 115.559 | failed |

全12試行中、出力確定までの完了は0回。[全測定記録](acceptance-2026-10-08/performance.json)にGNU time、RSSサンプル、停止理由、thread数、環境を保存した。停止理由は全件が外部wall観測上限で、メモリ不足による停止はなかった。

中断までのRSSは完走時の最大RSSの証明ではない。強制停止時刻は完了時間ではない。WSL系列はnative基準機の合格を証明しない。

## 検出・修正した不具合

- Bridgeが正当なmulticast MACを拒否していた入力判定。
- 時刻・hopの算術overflowが正しいE-0004診断にならない経路。
- 動的フィルタ理由の出力・Viewerの許容値不足。
- 接続済みswitch cycle診断でNEDのsource・target・spanが欠落する経路。
- 同時刻Controlの公開順と、CAN/Ethernetの未来Dispatch予約。
- 動的VIDの宣言上の登録能力をViewerが静的memberと同一視し、正常結果を拒否する問題。

修正前のViewer失敗は[元の失敗証跡](acceptance-2026-10-08/before-viewer-fix.json)、修正と再検証は[修正証跡](acceptance-2026-10-08/viewer-capability-fix.json)と最終catalogに残した。

## 追加の厳格な比較

Tだけを短くしたときの、終了時刻より前の観測についてevent_seq/effect_seqを含めて完全一致を要求する追加試験は、Bridge84行・AXI13行で不一致だった。物理時刻・値・順序との区別を[比較全行](acceptance-2026-10-08/prefix-probes.json)に保存した。このhorizon間の予約番号不変性は現行受入仕様に明記されていないため、規定のDIR-TEST-0022/0108をこの比較だけで不合格にしていない。設計上の判断を要する残事項として保持する。

## 受け入れ条件の個別判定

採用物台帳とmetadataへの実在参照、出自の全件照合、固定配布アーカイブの照合は未完了。native Ubuntuとの別環境比較、全故障注入点、破損FCSの公開入力なども未検証。各条件の具体的な不足、named test、source、対応の確度は[coverage review](acceptance-2026-10-08/coverage-review.json)を参照する。

| AC | 判定 |
| --- | --- |
| [DIR-AC-0001](../../要件定義書.md#dir-ac-0001) | 部分確認 |
| [DIR-AC-0002](../../要件定義書.md#dir-ac-0002) | 合格 |
| [DIR-AC-0003](../../要件定義書.md#dir-ac-0003) | 合格 |
| [DIR-AC-0004](../../要件定義書.md#dir-ac-0004) | 部分確認 |
| [DIR-AC-0005](../../要件定義書.md#dir-ac-0005) | 合格 |
| [DIR-AC-0006](../../要件定義書.md#dir-ac-0006) | 部分確認 |
| [DIR-AC-0007](../../要件定義書.md#dir-ac-0007) | 合格 |
| [DIR-AC-0008](../../要件定義書.md#dir-ac-0008) | 不合格 |
| [DIR-AC-0009](../../要件定義書.md#dir-ac-0009) | 不合格 |
| [DIR-AC-0010](../../要件定義書.md#dir-ac-0010) | 合格 |
| [DIR-AC-0011](../../要件定義書.md#dir-ac-0011) | 部分確認 |
| [DIR-AC-0012](../../要件定義書.md#dir-ac-0012) | 合格 |
| [DIR-AC-0013](../../要件定義書.md#dir-ac-0013) | 部分確認 |
| [DIR-AC-0014](../../要件定義書.md#dir-ac-0014) | 部分確認 |
| [DIR-AC-0015](../../要件定義書.md#dir-ac-0015) | 合格 |
| [DIR-AC-0016](../../要件定義書.md#dir-ac-0016) | 合格 |
| [DIR-AC-0017](../../要件定義書.md#dir-ac-0017) | 合格 |
| [DIR-AC-0018](../../要件定義書.md#dir-ac-0018) | 部分確認 |
| [DIR-AC-0019](../../要件定義書.md#dir-ac-0019) | 部分確認 |
| [DIR-AC-0020](../../要件定義書.md#dir-ac-0020) | 部分確認 |
| [DIR-AC-0021](../../要件定義書.md#dir-ac-0021) | 部分確認 |
| [DIR-AC-0022](../../要件定義書.md#dir-ac-0022) | 部分確認 |
| [DIR-AC-0023](../../要件定義書.md#dir-ac-0023) | 部分確認 |
| [DIR-AC-0024](../../要件定義書.md#dir-ac-0024) | 合格 |
| [DIR-AC-0025](../../要件定義書.md#dir-ac-0025) | 不合格 |
| [DIR-AC-0026](../../要件定義書.md#dir-ac-0026) | 部分確認 |
| [DIR-AC-0027](../../要件定義書.md#dir-ac-0027) | 部分確認 |
| [DIR-AC-0028](../../要件定義書.md#dir-ac-0028) | 合格 |
| [DIR-AC-0029](../../要件定義書.md#dir-ac-0029) | 合格 |
| [DIR-AC-0030](../../要件定義書.md#dir-ac-0030) | 部分確認 |
| [DIR-AC-0031](../../要件定義書.md#dir-ac-0031) | 部分確認 |
| [DIR-AC-0032](../../要件定義書.md#dir-ac-0032) | 部分確認 |
| [DIR-AC-0033](../../要件定義書.md#dir-ac-0033) | 部分確認 |
| [DIR-AC-0034](../../要件定義書.md#dir-ac-0034) | 部分確認 |
| [DIR-AC-0035](../../要件定義書.md#dir-ac-0035) | 合格 |
| [DIR-AC-0036](../../要件定義書.md#dir-ac-0036) | 部分確認 |
| [DIR-AC-0037](../../要件定義書.md#dir-ac-0037) | 部分確認 |
| [DIR-AC-0038](../../要件定義書.md#dir-ac-0038) | 部分確認 |
| [DIR-AC-0039](../../要件定義書.md#dir-ac-0039) | 部分確認 |
| [DIR-AC-0040](../../要件定義書.md#dir-ac-0040) | 部分確認 |
| [DIR-AC-0041](../../要件定義書.md#dir-ac-0041) | 部分確認 |
| [DIR-AC-0042](../../要件定義書.md#dir-ac-0042) | 部分確認 |
| [DIR-AC-0043](../../要件定義書.md#dir-ac-0043) | 部分確認 |
| [DIR-AC-0044](../../要件定義書.md#dir-ac-0044) | 部分確認 |
| [DIR-AC-0045](../../要件定義書.md#dir-ac-0045) | 部分確認 |
| [DIR-AC-0046](../../要件定義書.md#dir-ac-0046) | 部分確認 |
| [DIR-AC-0047](../../要件定義書.md#dir-ac-0047) | 部分確認 |
| [DIR-AC-0048](../../要件定義書.md#dir-ac-0048) | 部分確認 |
| [DIR-AC-0049](../../要件定義書.md#dir-ac-0049) | 部分確認 |
| [DIR-AC-0050](../../要件定義書.md#dir-ac-0050) | 部分確認 |
| [DIR-AC-0051](../../要件定義書.md#dir-ac-0051) | 部分確認 |
| [DIR-AC-0052](../../要件定義書.md#dir-ac-0052) | 部分確認 |
| [DIR-AC-0053](../../要件定義書.md#dir-ac-0053) | 部分確認 |
| [DIR-AC-0054](../../要件定義書.md#dir-ac-0054) | 部分確認 |
| [DIR-AC-0055](../../要件定義書.md#dir-ac-0055) | 部分確認 |
| [DIR-AC-0056](../../要件定義書.md#dir-ac-0056) | 部分確認 |
| [DIR-AC-0057](../../要件定義書.md#dir-ac-0057) | 部分確認 |
| [DIR-AC-0058](../../要件定義書.md#dir-ac-0058) | 部分確認 |
| [DIR-AC-0059](../../要件定義書.md#dir-ac-0059) | 部分確認 |
| [DIR-AC-0060](../../要件定義書.md#dir-ac-0060) | 部分確認 |
| [DIR-AC-0061](../../要件定義書.md#dir-ac-0061) | 部分確認 |
| [DIR-AC-0062](../../要件定義書.md#dir-ac-0062) | 部分確認 |
| [DIR-AC-0063](../../要件定義書.md#dir-ac-0063) | 部分確認 |
| [DIR-AC-0064](../../要件定義書.md#dir-ac-0064) | 部分確認 |

## 検証ケースの個別判定

| TEST | 判定 | 検証仕様 |
| --- | --- | --- |
| `DIR-TEST-0001` | 部分確認 | [CANモデル検証仕様書](../../verification/cases/CANモデル検証仕様書.md) |
| `DIR-TEST-0002` | 部分確認 | [CANモデル検証仕様書](../../verification/cases/CANモデル検証仕様書.md) |
| `DIR-TEST-0003` | 部分確認 | [CANモデル検証仕様書](../../verification/cases/CANモデル検証仕様書.md) |
| `DIR-TEST-0004` | 部分確認 | [CANモデル検証仕様書](../../verification/cases/CANモデル検証仕様書.md) |
| `DIR-TEST-0005` | 部分確認 | [CANモデル検証仕様書](../../verification/cases/CANモデル検証仕様書.md) |
| `DIR-TEST-0006` | 部分確認 | [CANモデル検証仕様書](../../verification/cases/CANモデル検証仕様書.md) |
| `DIR-TEST-0007` | 部分確認 | [共通実行検証仕様書](../../verification/cases/共通実行検証仕様書.md) |
| `DIR-TEST-0008` | 部分確認 | [共通実行検証仕様書](../../verification/cases/共通実行検証仕様書.md) |
| `DIR-TEST-0009` | 部分確認 | [共通実行検証仕様書](../../verification/cases/共通実行検証仕様書.md) |
| `DIR-TEST-0010` | 部分確認 | [共通実行検証仕様書](../../verification/cases/共通実行検証仕様書.md) |
| `DIR-TEST-0011` | 部分確認 | [GWモデル検証仕様書](../../verification/cases/GWモデル検証仕様書.md) |
| `DIR-TEST-0012` | 部分確認 | [GWモデル検証仕様書](../../verification/cases/GWモデル検証仕様書.md) |
| `DIR-TEST-0013` | 部分確認 | [GWモデル検証仕様書](../../verification/cases/GWモデル検証仕様書.md) |
| `DIR-TEST-0014` | 部分確認 | [GWモデル検証仕様書](../../verification/cases/GWモデル検証仕様書.md) |
| `DIR-TEST-0015` | 部分確認 | [GWモデル検証仕様書](../../verification/cases/GWモデル検証仕様書.md) |
| `DIR-TEST-0016` | 部分確認 | [GWモデル検証仕様書](../../verification/cases/GWモデル検証仕様書.md) |
| `DIR-TEST-0017` | 部分確認 | [AXIモデル検証仕様書](../../verification/cases/AXIモデル検証仕様書.md) |
| `DIR-TEST-0018` | 部分確認 | [AXIモデル検証仕様書](../../verification/cases/AXIモデル検証仕様書.md) |
| `DIR-TEST-0019` | 部分確認 | [AXIモデル検証仕様書](../../verification/cases/AXIモデル検証仕様書.md) |
| `DIR-TEST-0020` | 部分確認 | [AXIモデル検証仕様書](../../verification/cases/AXIモデル検証仕様書.md) |
| `DIR-TEST-0021` | 部分確認 | [AXIモデル検証仕様書](../../verification/cases/AXIモデル検証仕様書.md) |
| `DIR-TEST-0022` | 部分確認 | [AXIモデル検証仕様書](../../verification/cases/AXIモデル検証仕様書.md) |
| `DIR-TEST-0023` | 部分確認 | [Ethernetモデル検証仕様書](../../verification/cases/Ethernetモデル検証仕様書.md) |
| `DIR-TEST-0024` | 部分確認 | [Ethernetモデル検証仕様書](../../verification/cases/Ethernetモデル検証仕様書.md) |
| `DIR-TEST-0025` | 部分確認 | [Ethernetモデル検証仕様書](../../verification/cases/Ethernetモデル検証仕様書.md) |
| `DIR-TEST-0026` | 部分確認 | [Ethernetモデル検証仕様書](../../verification/cases/Ethernetモデル検証仕様書.md) |
| `DIR-TEST-0027` | 部分確認 | [Ethernetモデル検証仕様書](../../verification/cases/Ethernetモデル検証仕様書.md) |
| `DIR-TEST-0028` | 部分確認 | [Ethernetモデル検証仕様書](../../verification/cases/Ethernetモデル検証仕様書.md) |
| `DIR-TEST-0029` | 部分確認 | [拡張モデル統合検証仕様書](../../verification/cases/拡張モデル統合検証仕様書.md) |
| `DIR-TEST-0030` | 部分確認 | [拡張モデル統合検証仕様書](../../verification/cases/拡張モデル統合検証仕様書.md) |
| `DIR-TEST-0031` | 部分確認 | [拡張モデル統合検証仕様書](../../verification/cases/拡張モデル統合検証仕様書.md) |
| `DIR-TEST-0032` | 部分確認 | [拡張モデル統合検証仕様書](../../verification/cases/拡張モデル統合検証仕様書.md) |
| `DIR-TEST-0033` | 部分確認 | [Ethernet媒体拡張検証仕様書](../../verification/cases/Ethernet媒体拡張検証仕様書.md) |
| `DIR-TEST-0034` | 合格 | [Ethernet媒体拡張検証仕様書](../../verification/cases/Ethernet媒体拡張検証仕様書.md) |
| `DIR-TEST-0035` | 部分確認 | [Ethernet媒体拡張検証仕様書](../../verification/cases/Ethernet媒体拡張検証仕様書.md) |
| `DIR-TEST-0036` | 部分確認 | [Ethernet媒体拡張検証仕様書](../../verification/cases/Ethernet媒体拡張検証仕様書.md) |
| `DIR-TEST-0037` | 部分確認 | [Ethernet媒体拡張検証仕様書](../../verification/cases/Ethernet媒体拡張検証仕様書.md) |
| `DIR-TEST-0038` | 部分確認 | [Ethernet媒体拡張検証仕様書](../../verification/cases/Ethernet媒体拡張検証仕様書.md) |
| `DIR-TEST-0039` | 部分確認 | [Ethernet媒体拡張検証仕様書](../../verification/cases/Ethernet媒体拡張検証仕様書.md) |
| `DIR-TEST-0040` | 部分確認 | [CANFD・100BASE-T1検証仕様書](../../verification/cases/CANFD・100BASE-T1検証仕様書.md) |
| `DIR-TEST-0041` | 部分確認 | [CANFD・100BASE-T1検証仕様書](../../verification/cases/CANFD・100BASE-T1検証仕様書.md) |
| `DIR-TEST-0042` | 部分確認 | [CANFD・100BASE-T1検証仕様書](../../verification/cases/CANFD・100BASE-T1検証仕様書.md) |
| `DIR-TEST-0043` | 部分確認 | [CANFD・100BASE-T1検証仕様書](../../verification/cases/CANFD・100BASE-T1検証仕様書.md) |
| `DIR-TEST-0044` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0045` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0046` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0047` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0048` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0049` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0050` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0051` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0052` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0053` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0054` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0055` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0056` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0057` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0058` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0059` | 部分確認 | [メモリ・IPC検証仕様書](../../verification/cases/メモリ・IPC検証仕様書.md) |
| `DIR-TEST-0060` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0061` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0062` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0063` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0064` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0065` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0066` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0067` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0068` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0069` | 部分確認 | [入力・設定検証仕様書](../../verification/cases/入力・設定検証仕様書.md) |
| `DIR-TEST-0070` | 部分確認 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0071` | 合格 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0072` | 合格 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0073` | 合格 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0074` | 部分確認 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0075` | 部分確認 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0076` | 部分確認 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0077` | 部分確認 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0078` | 部分確認 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0079` | 部分確認 | [結果処理検証仕様書](../../verification/cases/結果処理検証仕様書.md) |
| `DIR-TEST-0080` | 部分確認 | [利用フロー・品質検証仕様書](../../verification/cases/利用フロー・品質検証仕様書.md) |
| `DIR-TEST-0081` | 部分確認 | [利用フロー・品質検証仕様書](../../verification/cases/利用フロー・品質検証仕様書.md) |
| `DIR-TEST-0082` | 部分確認 | [利用フロー・品質検証仕様書](../../verification/cases/利用フロー・品質検証仕様書.md) |
| `DIR-TEST-0083` | 部分確認 | [利用フロー・品質検証仕様書](../../verification/cases/利用フロー・品質検証仕様書.md) |
| `DIR-TEST-0084` | 不合格 | [利用フロー・品質検証仕様書](../../verification/cases/利用フロー・品質検証仕様書.md) |
| `DIR-TEST-0085` | 部分確認 | [利用フロー・品質検証仕様書](../../verification/cases/利用フロー・品質検証仕様書.md) |
| `DIR-TEST-0086` | 部分確認 | [利用フロー・品質検証仕様書](../../verification/cases/利用フロー・品質検証仕様書.md) |
| `DIR-TEST-0087` | 不合格 | [利用フロー・品質検証仕様書](../../verification/cases/利用フロー・品質検証仕様書.md) |
| `DIR-TEST-0090` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0091` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0092` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0093` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0094` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0095` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0096` | 部分確認 | [SoC・AHB・NoC検証仕様書](../../verification/cases/SoC・AHB・NoC検証仕様書.md) |
| `DIR-TEST-0097` | 部分確認 | [CANFD・100BASE-T1検証仕様書](../../verification/cases/CANFD・100BASE-T1検証仕様書.md) |
| `DIR-TEST-0098` | 部分確認 | [CANFD・100BASE-T1検証仕様書](../../verification/cases/CANFD・100BASE-T1検証仕様書.md) |
| `DIR-TEST-0099` | 部分確認 | [Ethernet負荷・QoS検証仕様書](../../verification/cases/Ethernet負荷・QoS検証仕様書.md) |
| `DIR-TEST-0100` | 部分確認 | [Ethernet負荷・QoS検証仕様書](../../verification/cases/Ethernet負荷・QoS検証仕様書.md) |
| `DIR-TEST-0101` | 部分確認 | [Ethernet負荷・QoS検証仕様書](../../verification/cases/Ethernet負荷・QoS検証仕様書.md) |
| `DIR-TEST-0102` | 部分確認 | [EthernetVLAN・マルチキャスト検証仕様書](../../verification/cases/EthernetVLAN・マルチキャスト検証仕様書.md) |
| `DIR-TEST-0103` | 部分確認 | [EthernetVLAN・マルチキャスト検証仕様書](../../verification/cases/EthernetVLAN・マルチキャスト検証仕様書.md) |
| `DIR-TEST-0104` | 部分確認 | [EthernetVLAN・マルチキャスト検証仕様書](../../verification/cases/EthernetVLAN・マルチキャスト検証仕様書.md) |
| `DIR-TEST-0105` | 合格 | [CAN・Ethernet変換検証仕様書](../../verification/cases/CAN・Ethernet変換検証仕様書.md) |
| `DIR-TEST-0106` | 部分確認 | [CAN・Ethernet変換検証仕様書](../../verification/cases/CAN・Ethernet変換検証仕様書.md) |
| `DIR-TEST-0107` | 部分確認 | [CAN・Ethernet変換検証仕様書](../../verification/cases/CAN・Ethernet変換検証仕様書.md) |
| `DIR-TEST-0108` | 部分確認 | [CAN・Ethernet変換検証仕様書](../../verification/cases/CAN・Ethernet変換検証仕様書.md) |
| `DIR-TEST-0109` | 部分確認 | [Ethernet動的制御検証仕様書](../../verification/cases/Ethernet動的制御検証仕様書.md) |
| `DIR-TEST-0110` | 部分確認 | [Ethernet動的制御検証仕様書](../../verification/cases/Ethernet動的制御検証仕様書.md) |
| `DIR-TEST-0111` | 部分確認 | [Ethernet動的制御検証仕様書](../../verification/cases/Ethernet動的制御検証仕様書.md) |
| `DIR-TEST-0112` | 部分確認 | [Ethernet動的制御検証仕様書](../../verification/cases/Ethernet動的制御検証仕様書.md) |
| `DIR-TEST-0113` | 部分確認 | [EthernetTSN検証仕様書](../../verification/cases/EthernetTSN検証仕様書.md) |
| `DIR-TEST-0114` | 部分確認 | [EthernetTSN検証仕様書](../../verification/cases/EthernetTSN検証仕様書.md) |
| `DIR-TEST-0115` | 部分確認 | [EthernetTSN検証仕様書](../../verification/cases/EthernetTSN検証仕様書.md) |
| `DIR-TEST-0116` | 部分確認 | [EthernetTSN検証仕様書](../../verification/cases/EthernetTSN検証仕様書.md) |

## 再実行

```bash
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release
python3 scripts/verification/acceptance_workflow.py --binary /absolute/pinned/dir-simulator --output /tmp/fresh-workflow
python3 scripts/verification/acceptance_catalog.py --binary /absolute/pinned/dir-simulator --output /tmp/fresh-catalog
python3 scripts/verification/acceptance_prefix.py --binary /absolute/pinned/dir-simulator --output /tmp/fresh-prefix
python3 scripts/performance/measure_can_million.py --binary /absolute/pinned/dir-simulator --inputs docs/verification/fixtures/performance/can-million --output /tmp/fresh-million --wall-limit-seconds 180
```

prefix scriptのexit1は追加比較の不一致を表す。性能収集scriptのexit0は記録処理の完了であり、性能合格はmeasurement.jsonのevaluationで判定する。ブラウザと性能測定を同時に走らせない。公開入力・source/fixture hashは各JSONに保存した。完了したcatalog/workflowの生出力は記載したローカル作業領域に保持した。性能試行の未確定spoolは一覧とbyte数を記録した後に削除し、84個の測定記録・ログ・RSSサンプルを同梱した。Input Aの40出力ファイルも同梱した。
