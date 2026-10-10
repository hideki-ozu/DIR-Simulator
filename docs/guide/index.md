# DIR Simulator ガイド

文書バージョン：`1.1.7`  
対象GitHubバージョン：CAN記事は`v1.1.3`、Ethernet期限・Gateway記事は`v1.1.4`  
文書ID：`guide-index`  
文書状態：公開用完成稿。mainへのマージ後にGitHub Pagesへ反映

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
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

DIRを初めて使う人が、サンプルのCAN・Ethernet通信を実行し、結果を読み、設定を変えて理由を確かめるためのガイドです。既存CAN記事のコードと数値の基準は公開版v1.1.3（`dd6199503913db24729f04bcca7e9e4f856fd71c`）です。Ethernet配送期限の記事はv1.1.3の入力を基礎に、公開版v1.1.4の固定sourceで実測しています。

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
| 設定・用語・FAQ | [設定・用語・FAQ](設定・用語・FAQ.md) | パス、単位、上書きエラー、学習時の疑問 |

上から3ページを順に読むと、一つの小さな実験がつながります。入門は3要求だけに絞り、実際に保存された結果と画面を掲載しています。

## 現在使える範囲

入門・Viewer・調停・受信フィルタ・受信処理遅延の実習はClassical CANの`can.cc.ideal.v1`です。「CAN FDのデータ速度比較」は、外部から与える位相bit数を使う`can.fd.precomputed.v1`の実習です。Ethernet配送期限の実習は`ethernet.l2.qos.v1`です。Gateway処理遅延の実習は`can.cc.multibus.v1`を正式v1.1.4で実測し、別の[Gateway実習入力ZIP](downloads/gateway-delay-inputs.zip)を使います。現行ソースには複数CANバスとGateway、EthernetとQoS/VLAN/媒体拡張、外部計算位相bit数を使うCAN FD、AXI/SoC/AHB/NoC/メモリ・IPCの初期transaction profile、NED editorとViewerもあります。READMEの古い「予定」表記だけでは実装有無を判定しません。[v1.1.3 Release](https://github.com/hideki-ozu/DIR-Simulator/releases/tag/v1.1.3)と対象タグの実装を基準にします。

規格全体への適合や100万要求の性能受入は、この実習が証明する範囲に含みません。CAN FDは外部算定位相bit数を用いる`structural-only`の時間評価で、任意フレームの完全wire codecや完全ISO波形適合を保証しません。
