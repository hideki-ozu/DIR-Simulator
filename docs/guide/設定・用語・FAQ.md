# 設定・用語・FAQ

文書バージョン：`1.1.0`  
対象GitHubバージョン：`v1.1.3`  
文書ID：`guide-reference`  
文書状態：公開用完成稿。mainへのマージ後にGitHub Pagesへ反映

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-05` | 初版：公開サンプルの実行・実画面・条件変更実験、GitHub Pages公開とMarkdown再利用を整備 |

## 最小例で使う設定

| 設定 | 意味 | 今回の値・注意 |
| --- | --- | --- |
| network | NEDのnetwork型 | demo.Main |
| ned-path | NED探索場所 | INIからの相対パスmodels |
| workload | 生成入力 | INIからの相対パスminimal.json |
| sim-time-limit | 観測終了 | 1ms。内部時刻はps整数 |
| metrics-window | 窓ごとの集計幅 | 1ms |
| Main.bus.bitrate | Busの速度 | 500kbps / 1Mbps |
| Main.*.queueCapacity | 待機するTX要求の容量 | 各64。送信中要求は待機キューから外れる |

INIはNEDのパラメータを上書きします。NED自体は独立パーサで対応サブセットを扱うため、任意のOMNeT++プロジェクトがそのまま動く互換環境とは説明しません。

## 用語

| 用語 | 読み方 |
| --- | --- |
| CAN ID | フレームの識別子・調停優先順位。最小例では256など |
| generator ID | workloadの生成器ID。aなど |
| request ID | 生成された要求の追跡ID。a:0など |
| SOF / EOF | フレーム送信開始 / フレーム終端 |
| release | 3bit間隔を終えてBusが解放される時刻 |
| observed / received | 受信観測 / フィルタ通過後のRX処理完了 |
| generated / ready | 要求生成 / 送信可能になる時刻 |
| schema / profile | 出力形式の版 / 扱うモデルの契約 |
| partial | 途中状態を含む結果。未確定を完了として補わない |

## よくある疑問

### runやviewが既存出力を拒否する

既存データを保護するためです。出力先を`results-guide-minimal-2`や`guide-minimal-viewer-2.html`に変えて再実行します。入力JSONを編集して時刻を直す方法は使いません。

### フレームは3つなのに受信が6件ある

送信元以外の2ノードがそれぞれ受信するからです。送信要求数と受信機会を分けます。受信フィルタを変えれば受信件数は変わります。

### EOFと次のSOFが一致しない

最小例では3bitの間隔があります。500kbpsなら6µsです。EOFで送信成功が確定しても、Bus解放までは次の送信を始めません。

### 1ms実行なのに最後の通信は536µs

各ノード1回しか生成しないため、後続イベントがなくなります。観測終了は1msのままです。バス占有率やthroughputの分母も、最後のEOFだけへ短縮して解釈しないでください。

### Viewerの検索・省略で結果件数が変わるか

一覧の表示条件が変わるだけです。集計の母数は全記録です。要求の時刻列は実行全体、状態は選択時刻の値です。

### Windowsで同じコマンドを使えるか

掲載Bashコマンドと実行検証はWSLで行いました。Viewer HTMLとMkDocs静的ガイドはWindowsブラウザで読めます。Windows native Rustビルドは今回検証していません。

### 実装とREADMEの予定表記が違う

対象タグの実装・テスト・Releaseを確認します。v1.1.3には5つの初期transaction profileが入り、Viewerも対応しています。全文規格適合や性能受入と、profileが動くことは別の確認です。

[入門](初めてのCAN実行.md)、[画面操作](Viewerで結果を読む.md)、[調停実験](CANの調停.md)へ戻れます。
