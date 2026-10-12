# DIR Simulator ガイド

文書バージョン：`1.1.13`
対象GitHubバージョン：`v1.1.4`
文書ID：`guide-index`  
文書状態：公開済み。mainへのpush時に検証後、GitHub Pagesを自動更新

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.13` | `2026-10-12` | AXI書き込みストローブの独立実習・8入力ZIP・実Viewer・metadata/ページ数検査を追加 |
| `1.1.12` | `2026-10-11` | DDRリフレッシュ時間の独立実習・8入力ZIP・実Viewer・fresh証跡検証を追加 |
| `1.1.11` | `2026-10-11` | 公開文書整合性監査の修正を反映し、文書PATCHを一つ上げ、対象タグをv1.1.4へ統一 |
| `1.1.10` | `2026-10-11` | DMAのコピー完了と通知待ちの独立実習・8入力ZIP・fresh証跡検証を追加 |
| `1.1.9` | `2026-10-11` | メールボックス容量・満杯応答の独立実習と8入力ZIPを追加。既存入口・履歴を保持 |
| `1.1.8` | `2026-10-11` | SRAMポート数の独立実習と8入力ZIPを追加。既存入口と履歴を保持 |
| `1.1.7` | `2026-10-11` | Gateway処理遅延の独立実習と9入力ZIPを追加。既存CC/FD/Ethernet入口と分岐履歴を保持 |
| `1.1.6` | `2026-10-10` | PR #36第2push。最新mainのEthernet入口を保持し、Classical CAN/FDの独立ZIPとFD profile・制約を明記。両分岐の1.1.5履歴を保持 |
| `1.1.5` | `2026-10-07` | Classical CANとCAN FDの実習profile・配布入力を区別し、ガイド全体の対象範囲説明を修正（PR #36の分岐履歴） |
| `1.1.5` | `2026-10-10` | Ethernet期限の実測比較を追加し、CAN記事の基準版と実測版を区別 |
| `1.1.4` | `2026-10-07` | CAN FDのデータ速度比較と独立配布入力を追加 |
| `1.1.3` | `2026-10-06` | 公開済みmainを統合し、受信フィルタと受信処理遅延の両ガイド・配布入力への入口を維持 |
| `1.1.2` | `2026-10-06` | CANの受信フィルタとECU別の受信選択実習を追加 |
| `1.1.2` | `2026-10-06` | CAN受信処理遅延の1変数比較ガイドと配布入力を追加し、送信成功・観測・受信完了・終了時pendingへの入口を整備 |
| `1.1.1` | `2026-10-06` | 読者向けガイドから原稿再利用と執筆管理の説明を分離 |
| `1.1.0` | `2026-10-05` | 初版：公開サンプルの実行・実画面・条件変更実験、GitHub Pages公開とMarkdown再利用を整備 |

DIRを初めて使う人が、サンプルのCAN・Ethernet通信とメモリ読出し・IPCを実行し、結果を読み、設定を変えて理由を確かめるためのガイドです。既存CAN記事のコードと数値の基準は公開版v1.1.3（`dd6199503913db24729f04bcca7e9e4f856fd71c`）です。Ethernet配送期限の記事はv1.1.3の入力を基礎に、公開版v1.1.4の固定sourceで実測しています。

## ここから始める

[Classical CAN実習入力ZIP](downloads/can-guide-inputs.zip)の導入手順は「初めてのCAN実行」に記載しています。CAN FDは別の[FD実習入力ZIP](downloads/canfd-guide-inputs.zip)を使い、「CAN FDのデータ速度比較」の手順で実行します。

| 読みたいこと | 完成しているページ | 得られること |
| --- | --- | --- |
| 入門 | [初めてのCAN実行](初めてのCAN実行.md) | NED・INI・JSONの役割、3要求の実行、ビットレート実験 |
| 画面操作 | [Viewerで結果を読む](Viewerで結果を読む.md) | 時刻移動、タイムライン、詳細、実績と予定の違い |
| 機能辞典・目的別実験 | [CANの調停](CANの調停.md) | 優先順位と待ち時間、CAN IDを1つ変える実験 |
| 機能辞典・受信選択 | [CANの受信フィルタ](CANの受信フィルタ.md) | 送信成功と受信選別を区別し、1設定を3条件で比較 |
| 受信処理の実験 | [CAN受信処理遅延の比較](CAN受信処理遅延の比較.md) | 受信処理時間だけを変え、送信・観測・受信完了と終了時pendingを区別 |
| FDデータ速度の実験 | [CAN FDのデータ速度比較](CANFDのデータ速度比較.md) | 位相bit数を固定し、速度と次要求の待ち時間を比較 |
| Ethernet期限の実験 | [Ethernet配送期限と遅延判定の比較](Ethernet配送期限と遅延判定の比較.md) | deadlineだけを1psずつ変え、同じ配送遅延と期限超過判定を区別 |
| 複数CANバスの実験 | [Gateway処理遅延と複数CANバスの中継](Gateway処理遅延と複数CANバスの中継.md) | 処理遅延だけを変え、元通信・二つのコピー・RX保持を区別 |
| SRAM読出しの実験 | [SRAMのポート数と読み出し待ち時間](SRAMのポート数と読み出し待ち時間.md) | ポート数だけを変え、待ち時間と処理時間を区別 |
| IPC受渡しの実験 | [メールボックスの容量と満杯応答](メールボックスの容量と満杯応答.md) | メッセージ容量だけを変え、完了・full/empty・FIFO受信・通知を区別 |
| DMAコピーの実験 | [DMAのコピー完了と通知待ち](DMAのコピー完了と通知待ち.md) | 通知時間だけを変え、書き込み完了と通知待ちを区別 |
| DDRの実験 | [DDRのリフレッシュと読み出し待ち](DDRのリフレッシュと読み出し待ち.md) | refresh時間だけを変え、開行リセットと読み出し待ちを区別 |
| AXI部分更新の実験 | [AXIの書き込みストローブと部分更新](AXIの書き込みストローブと部分更新.md) | WSTRBだけを変え、五チャネルの完了とRAM byte更新を区別 |
| 設定・用語・FAQ | [設定・用語・FAQ](設定・用語・FAQ.md) | パス、単位、上書きエラー、学習時の疑問 |

上から3ページを順に読むと、一つの小さな実験がつながります。入門は3要求だけに絞り、実際に保存された結果と画面を掲載しています。

## 現在使える範囲

入門・Viewer・調停・受信フィルタ・受信処理遅延の実習はClassical CANの`can.cc.ideal.v1`です。「CAN FDのデータ速度比較」は、外部から与える位相bit数を使う`can.fd.precomputed.v1`の実習です。Ethernet配送期限の実習は`ethernet.l2.qos.v1`です。Gateway処理遅延の実習は`can.cc.multibus.v1`を正式v1.1.4で実測し、別の[Gateway実習入力ZIP](downloads/gateway-delay-inputs.zip)を使います。SRAMの実習は`memory.ipc.transaction.v1`を正式v1.1.4で実測し、[SRAM実習入力ZIP](downloads/sram-ports-inputs.zip)を使います。メールボックスの実習も`memory.ipc.transaction.v1`を正式v1.1.4で実測し、[独立入力ZIP](downloads/mailbox-capacity-inputs.zip)を使います。現行ソースには複数CANバスとGateway、EthernetとQoS/VLAN/媒体拡張、外部計算位相bit数を使うCAN FD、AXI/SoC/AHB/NoC/メモリ・IPCの初期transaction profile、NED editorとViewerもあります。実習入力は各ページの配布ZIPまたはmainの`examples/guide`から取得します。profileの提供状態は各記事の対象タグ（[v1.1.3 Release](https://github.com/hideki-ozu/DIR-Simulator/releases/tag/v1.1.3)／[v1.1.4 Release](https://github.com/hideki-ozu/DIR-Simulator/releases/tag/v1.1.4)）と実装で確認してください。

規格全体への適合や100万要求の性能受入は、この実習が証明する範囲に含みません。CAN FDは外部算定位相bit数を用いる`structural-only`の時間評価で、任意フレームの完全wire codecや完全ISO波形適合を保証しません。

DMAの実習は正式v1.1.4の `memory.ipc.transaction.v1` を使います。[DMA実習入力ZIP](downloads/dma-notification-inputs.zip)で再現できます。

DDRの実習も正式v1.1.4の `memory.ipc.transaction.v1` を使い、[独立DDR入力ZIP](downloads/ddr-refresh-inputs.zip)で再現できます。
