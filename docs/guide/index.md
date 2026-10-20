# DIR Simulator ガイド

文書バージョン：`1.1.31`
対象GitHubバージョン：`v1.1.6`
文書ID：`guide-index`  
文書状態：公開済み。mainへのpush時に検証後、GitHub Pagesを自動更新

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.31` | `2026-10-20` | Issue #109・#115の対応と、正式v1.1.6で実測した半二重再送・AXI容量・TAS/CBS・Gateway RX容量の4実習への入口を追加 |
| `1.1.30` | `2026-10-19` | preemption・冗長履歴・PSFP gate・Metric V2の正式v1.1.6実測4記事と独立入力ZIPを追加 |
| `1.1.29` | `2026-10-17` | 機能リファレンス8章と最新mainの5記事を共存させ、v1.1.6の入口・入力ZIP・過去実測の基準を整理 |
| `1.1.28` | `2026-10-17` | 抽象STPのcost1変数比較・正式版実測・実Viewerと6入力ZIPを追加。既存記事・導線・23 ZIPを維持 |
| `1.1.27` | `2026-10-16` | 動的VID登録の寿命1変数比較・正式版実測・実Viewerと6入力ZIPを追加。既存記事・導線・22 ZIPを維持 |
| `1.1.26` | `2026-10-16` | 源別multicastのmode1変数比較・正式版実測・実Viewerと6入力ZIPを追加。既存記事・導線・21 ZIPを維持 |
| `1.1.25` | `2026-10-16` | link復旧時刻の1ps比較・正式版実測・実Viewerと6入力ZIPを追加。既存記事・共通導線・20 ZIPを維持 |
| `1.1.24` | `2026-10-15` | MAC学習のaging1変数比較・正式版実測・実Viewerと6入力ZIPを追加。既存記事・ZIP・共通導線を維持 |
| `1.1.23` | `2026-10-15` | PSFPのyellow_action1変数比較・正式版実測・実Viewerと6入力ZIPを追加。既存記事・ZIP・履歴を保持 |
| `1.1.22` | `2026-10-15` | CBSのidle slope1変数比較・正式版実測・実Viewerと8入力ZIPを追加。PR97と既存記事・ZIP・履歴を保持 |
| `1.1.21` | `2026-10-14` | 静的multicastの出口リスト1変数比較・正式版実測・実Viewerと9入力ZIPを追加。既存記事・ZIP・履歴を保持 |
| `1.1.20` | `2026-10-14` | Ethernet TASの生成時刻1ps境界実習・IFG込み窓末尾・実Viewerと8入力ZIPを追加。既存20記事と履歴を保持 |
| `1.1.19` | `2026-10-14` | Ethernet VLAN受入admitだけを変える8入力実習・filtered理由・正式版実測と実Viewerを追加。未マージ4記事を保持 |
| `1.1.18` | `2026-10-13` | Ethernetの論理priorityだけを変える8入力実習・非プリエンプト/FIFO・実Viewerを追加。既存3記事を保持 |
| `1.1.17` | `2026-10-13` | NoC入力FIFOの独立8入力実習・35ps実Viewer・開始待ちとhop間待ちを追加。SoC/AHBを保持 |
| `1.1.16` | `2026-10-13` | AHBの応答待ちの独立実習・8入力ZIP・ERROR完了・実Viewerを追加。SoC成果物を保持 |
| `1.1.15` | `2026-10-13` | SoC共有バスの調停の独立実習・6入力ZIP・実Viewer・metadata/nav検査を追加 |
| `1.1.14` | `2026-10-12` | 第3push。最新mainを通常取り込みし共有メモリとAXIの入口・ZIP・両分岐履歴を統合。nav17/HTML18を明記 |
| `1.1.13` | `2026-10-12` | 共有メモリの取得actorだけを変える独立実習・8入力ZIP・実Viewer・metadata検査を追加 |
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

## 機能の採否・仕様変更を検討する

[DIR機能・動作仕様ガイド](reference/DIR機能・動作仕様ガイド.md)は、mainの固定コミットを対象に、機能と動作を概要／詳細で説明する調査資料です。共通基盤、NEDエディタとViewer、16組込みprofileの設定・固定値・関連規格・変更判断を、実画面と参照元付きで読めます。基準版と今回の実行範囲は資料内に明記しています。

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
| 共有メモリIPCの実験 | [共有メモリの公開と取得権限](共有メモリの公開と取得権限.md) | consumeのactorだけを変え、公開・取得・拒否とslotの状態を区別 |
| SoC共有バスの実験 | [SoC共有バスの調停と要求の待ち時間](SoC共有バスの調停と要求の待ち時間.md) | 調停だけを変え、source FIFO・占有と個別/平均の待ちを比較 |
| AHB応答待ちの実験 | [AHBの応答待ちとERROR完了](AHBの応答待ちとERROR完了.md) | wait_cyclesだけを変え、開始待ち・占有とERROR完了を区別 |
| NoC入力FIFOの実験 | [NoCの入力FIFO容量と待ち時間](NoCの入力FIFO容量と待ち時間.md) | input_capacityだけを変え、最初のgrant・hop間待ちと最終配送を区別 |
| Ethernet優先キューの実験 | [Ethernetの優先キューと送信順](Ethernetの優先キューと送信順.md) | priorityだけを変え、FIFO・非プリエンプトとflow別待ちを区別 |
| Ethernet VLANの実験 | [EthernetのVLAN受入条件と破棄理由](EthernetのVLAN受入条件と破棄理由.md) | admitだけを変え、受入拒否の場所・時刻・理由を読む |
| Ethernet TASの実験 | [Ethernet TASの送信窓とIFG込みの待ち時間](<Ethernet TASの送信窓とIFG込みの待ち時間.md>) | 生成時刻だけを1psずつ変え、送信窓の等号と次窓への待ちを読む |
| Ethernet multicastの実験 | [Ethernetの静的multicastとコピーの配送先](Ethernetの静的multicastとコピーの配送先.md) | 静的groupの出口リストだけを変え、転送コピー・経路とEndpoint購読を区別 |
| Ethernet CBSの実験 | [Ethernet CBSのcredit回復と送信待ち](<Ethernet CBSのcredit回復と送信待ち.md>) | idle slopeだけを変え、物理Link占有とcredit回復の待ちを区別 |
| Ethernet PSFPの実験 | [Ethernet PSFPの色判定とyellow破棄](<Ethernet PSFPの色判定とyellow破棄.md>) | yellow_actionだけを変え、色判定・token消費と受信処置を区別 |
| Ethernet MAC学習の実験 | [EthernetのMAC学習とagingによる転送先](EthernetのMAC学習とagingによる転送先.md) | mac_age_psだけを変え、学習期限・既知unicastとfloodを区別 |
| link復旧と受信の境界を調べたい | [Ethernetのlink復旧時刻と受信判定](Ethernetのlink復旧時刻と受信判定.md) | 復旧時刻だけを1ps変え、送信継続と受信filterを分ける |
| 源別multicastの実験 | [Ethernetの源別multicastとINCLUDE・EXCLUDE](Ethernetの源別multicastとINCLUDE・EXCLUDE.md) | modeだけを変え、送信元集合・候補0・期限後のfloodを区別 |
| VID登録の期限を比較する | [Ethernetの動的VID登録と有効期限](Ethernetの動的VID登録と有効期限.md) | 出口portのlifetimeだけを変え、対向portと有効VIDを区別 |
| 抽象STPの経路を比べる | [Ethernetの抽象STPとリンクcostによる配送経路](Ethernetの抽象STPとリンクcostによる配送経路.md) | L12 costだけを変え、初期role・枝のcopyと配送時間を区別 |
| fragment長と待ちを比べる | [抽象preemptionのfragment長とexpress待ち時間](抽象preemptionのfragment長とexpress待ち時間.md) | fragment長だけを変え、express待ち・slow占有・IFGと再開gapを区別 |
| 冗長コピーの除去理由を読む | [抽象冗長配送の履歴容量とduplicate・stale](抽象冗長配送の履歴容量とduplicate・stale.md) | 履歴容量だけを変え、物理受信・一度だけの配送・duplicateとstaleを区別 |
| 受信gateの境界を比べる | [Ethernet PSFPのstream gateと1ps境界](<Ethernet PSFPのstream gateと1ps境界.md>) | gate開始だけを1ps変え、同じ送信copyと異なる受信判定を読む |
| 集計窓と時間平均を読む | [Metric V2の集計窓幅と時間平均](<Metric V2の集計窓幅と時間平均.md>) | 窓幅だけを変え、同じイベント列・窓別の時間平均・現在値を区別 |
| 半二重媒体の再送を比べる | [半二重Ethernetの再送とseedによる待ち時間の違い](<半二重Ethernetの再送とseedによる待ち時間の違い.md>) | seedだけを変え、衝突・backoff期限・carrier待ちと同条件の再現を確認 |
| AXIの受理容量を比べる | [AXIのoutstanding上限と受理・待機・処理の違い](<AXIのoutstanding上限と受理・待機・処理の違い.md>) | outstanding上限だけを変え、drop・FIFO待機と一要求ずつの処理を区別 |
| TASとCBSの合成を読む | [TASのgate閉鎖とCBS creditの凍結](<TASのgate閉鎖とCBS creditの凍結.md>) | gate閉鎖だけを変え、負creditの凍結と再開後の回復待ちを比較 |
| GatewayのRX容量を比べる | [GatewayのRX容量と遅い分岐による後続配信の損失](<GatewayのRX容量と遅い分岐による後続配信の損失.md>) | RX容量だけを変え、親保持・枝別TX受理と後続親の拒否を区別 |
| 設定・用語・FAQ | [設定・用語・FAQ](設定・用語・FAQ.md) | パス、単位、上書きエラー、学習時の疑問 |

上から3ページを順に読むと、一つの小さな実験がつながります。入門は3要求だけに絞り、実際に保存された結果と画面を掲載しています。

## 現在使える範囲

入門・Viewer・調停・受信フィルタ・受信処理遅延の実習はClassical CANの`can.cc.ideal.v1`です。「CAN FDのデータ速度比較」は、外部から与える位相bit数を使う`can.fd.precomputed.v1`の実習です。Ethernet配送期限の実習は`ethernet.l2.qos.v1`です。Gateway処理遅延の実習は`can.cc.multibus.v1`を正式v1.1.4で実測し、別の[Gateway実習入力ZIP](downloads/gateway-delay-inputs.zip)を使います。SRAMの実習は`memory.ipc.transaction.v1`を正式v1.1.4で実測し、[SRAM実習入力ZIP](downloads/sram-ports-inputs.zip)を使います。メールボックスの実習も`memory.ipc.transaction.v1`を正式v1.1.4で実測し、[独立入力ZIP](downloads/mailbox-capacity-inputs.zip)を使います。現行ソースには複数CANバスとGateway、EthernetとQoS/VLAN/媒体拡張、外部計算位相bit数を使うCAN FD、AXI/SoC/AHB/NoC/メモリ・IPCの初期transaction profile、NED editorとViewerもあります。実習入力は各ページの配布ZIPまたはmainの`examples/guide`から取得します。profileの提供状態は各記事の対象タグ（[v1.1.3 Release](https://github.com/hideki-ozu/DIR-Simulator/releases/tag/v1.1.3)／[v1.1.4 Release](https://github.com/hideki-ozu/DIR-Simulator/releases/tag/v1.1.4)）と実装で確認してください。

規格全体への適合や100万要求の性能受入は、この実習が証明する範囲に含みません。CAN FDは外部算定位相bit数を用いる`structural-only`の時間評価で、任意フレームの完全wire codecや完全ISO波形適合を保証しません。

DMAの実習は正式v1.1.4の `memory.ipc.transaction.v1` を使います。[DMA実習入力ZIP](downloads/dma-notification-inputs.zip)で再現できます。

DDRの実習も正式v1.1.4の `memory.ipc.transaction.v1` を使い、[独立DDR入力ZIP](downloads/ddr-refresh-inputs.zip)で再現できます。

共有メモリの実習も正式v1.1.4の `memory.ipc.transaction.v1` を使い、[共有メモリ入力ZIP](downloads/shared-roles-inputs.zip)で再現できます。

VLAN受入条件の実習は正式v1.1.4の`ethernet.l2.vlan.v1`を使い、[独立入力ZIP](downloads/ethernet-vlan-inputs.zip)で再現できます。

TAS送信窓の実習は正式v1.1.4の`ethernet.tsn.v1`を使い、[独立入力ZIP](downloads/ethernet-tas-inputs.zip)で再現できます。

静的multicastの実習は正式v1.1.4の`ethernet.l2.vlan.v1`を使い、[独立入力ZIP](downloads/ethernet-multicast-inputs.zip)で再現できます。

CBSの実習は正式v1.1.4の`ethernet.tsn.v1`を使い、[独立8入力ZIP](downloads/ethernet-cbs-inputs.zip)で再現できます。

PSFPの実習も正式v1.1.4の`ethernet.tsn.v1`を使い、[独立6入力ZIP](downloads/ethernet-psfp-inputs.zip)で再現できます。

MAC学習の実習は正式v1.1.4の`ethernet.l2.dynamic.v1`を使い、[独立6入力ZIP](downloads/ethernet-learning-inputs.zip)で再現できます。

link復旧の実習は正式v1.1.4の`ethernet.l2.dynamic.v1`を使い、[独立6入力ZIP](downloads/ethernet-link-control-inputs.zip)で再現できます。

源別multicastの実習は正式v1.1.4の`ethernet.l2.dynamic.v1`を使い、[独立6入力ZIP](downloads/ethernet-source-filter-inputs.zip)で再現できます。

動的VID登録の実習は正式v1.1.4の`ethernet.l2.dynamic.v1`を使い、[独立6入力ZIP](downloads/ethernet-vid-registration-inputs.zip)で再現できます。

抽象STPの実習は正式v1.1.4の`ethernet.l2.dynamic.v1`を使い、[独立6入力ZIP](downloads/ethernet-stp-cost-inputs.zip)で再現できます。

抽象preemptionと冗長配送の実習は正式v1.1.6の`ethernet.preemption.abstract.v1`／`ethernet.redundancy.abstract.v1`を使い、[fragment比較ZIP](downloads/ethernet-preemption-fragments-inputs.zip)／[履歴容量比較ZIP](downloads/ethernet-redundancy-history-inputs.zip)で再現できます。PSFP gateの実習も正式v1.1.6の`ethernet.tsn.v1`を使い、[gate境界ZIP](downloads/ethernet-psfp-gate-inputs.zip)を配布しています。

Metric V2の実習は正式v1.1.6に同梱されたRust exampleの独自登録profile`demo.control.v1`です。通常CLIのrunでは実行できません。[独立入力ZIP](downloads/metric-windows-inputs.zip)と正式版のexampleを使う手順を記事内に記載しています。
