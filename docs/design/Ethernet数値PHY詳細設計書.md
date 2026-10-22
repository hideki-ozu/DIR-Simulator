# Ethernet数値PHY詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-numerical-phy`
文書状態：未commit・未公開。project数値契約と別文書のIEEE選択契約を区別する。

<a id="samples"></a>

```trace
{"id":"design-numerical-phy#samples","stage":"design","requirements":["DIR-REQ-0303"],"upstream":["architecture#numerical-phy"],"state":"confirmed","pending":[]}
```


### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 数値sampleから実MAC復号までの実行・保存契約を追加 |

`ethernet.phy.v1` は、実際の MAC バイトを電圧サンプルへ変換し、伝搬チャネルと受信器を通して復元する登録モデルである。本書は `execution_contract = "dir.sampled-manchester.v1"`、`standard_edition = "project.numerical.v1"` のproject数値契約を規定する。別のexecution contractで実行するIEEE選択PHYは[専用設計](IEEE選択PHY詳細設計書.md)を参照する。projectのManchester波形からIEEEのPCSや電気的適合を推定しない。従来の Ethernet プロファイルは既存の実行経路を維持する。

## バイトと物理サンプル

送信入力は DA、SA、任意の C-VLAN/R-TAG、EtherType、データとパディング、wire-order FCS を含む実際の MAC バイトであり、最短64バイトである。送信時に長さと CRC-32 を検証する。物理経路は、16バイトのプロジェクト訓練パイロット `55`、7バイトのプリアンブル `55` と SFD `d5`、MAC バイトを順に送る。各バイトは LSB first、データ0は `[-1,+1]`、データ1は `[+1,-1]` の2チップへ変換する。この符号とパイロットはプロジェクト固有であり、T1 PCS の代替符号ではない。

チップを TX の整数µV電圧、線形立上り/立下りと Q30 FIR へ入力する。サンプル時刻は `index × 10^12 / (symbol_rate_hz × samples_per_ui)` ps の正確な有理数であり、`symbol_rate_hz` はこの契約ではチップレートである。共通 FES へは各サンプル終端の切上げps時刻を渡し、分数位相を消さない。N×N の因果的 FIR 行列が伝搬、反射、自己結合を表す。全経路を checked i128 で加算し、一度だけ丸めとクリッピングを行う。既定のサンプル例はノイズ無しである。ノイズ設定時は endpoint ID に依存する SplitMix64/Box–Muller の状態と余剰正規乱数を保存する。同一実行ターゲットの決定性を契約とし、未検証のプラットフォーム間ビット一致は主張しない。

受信器は実サンプルへのフロントエンド FIR、ADC、既知のパイロットによる NLMS、測定誤差によるロック、Gardner 位相/周波数追従を用いる。相手の実際の ppm や送信データを受信器へ渡さない。半UIの追従には偶数の samples/UI が必要である。FFE と既知ローカル echo の基底はシンボル間隔であり、任意の分数UI echo に対する厳密な消去性能は契約しない。

Manchester デコーダは復元チップから SFD を探索し、ロックした受信器の実ビットのみで MAC バイトを組み立てる。実際の無信号区間まで受信を継続してフレーム終端を確定し、部分バイト、符号違反、長さ、FCSを検査する。受信成立を送信予定EOF、期待ペイロードや期待長との一致で決定しない。受信器は frame token の epoch/serial を保持し、ロック喪失で現在の受信を破棄する。後の再訓練で古い完了を成立させない。

## 共有媒体と MAC

1つの `SampledMedium` が全 endpoint の同時送信決定、全電圧の解決、各 endpoint の観測の順で1サンプルを処理する。carrier は各 endpoint に到着した電圧と閾値/hold時間から導出する。グローバルな busy 状態で送信を一括抑止しない。

`half_duplex=true` では、実際に到着した受信電圧から設定済み自己経路の電圧を差し引いた残差を衝突検出へ使う。衝突後は現在のチップを完了し、将来チップを実際の jam 波形へ置換する。MAC ごとに retry count、ID/seed による BEB draw、残りの実アイドルサンプル数を保持する。carrier 中はアイドル/バックオフの進行を止める。`retry_limit` は最初の試行に加えて許す再試行数である。late は設定 slot 時間を越えた衝突を示し、このプロジェクト契約では通常の有限 retry policy に従う。IEEE Clause4 の適合性は表明しない。

`half_duplex=false` は、明示的に自己結合無しとした独立方向の数値モデルである。同じ endpoint の送受信を並行して処理する。自己結合を指定したこのモードは拒否する。スカラー電圧の相殺や閾値の限界を含む宣言済み物理モデルであり、全ての配線/位相について標準の collision assertion 要件を満たすと主張しない。標準 repeater、DME、PLCA の動作を推測して追加しない。

`TxFinished` は最後の送信チップが実際に出力されたことを示す。`TxReady` は実際の IFG、carrier、backoff 条件から導出する。source のキュー予約は ready まで保持し、実際の受信完了と区別する。IFG中、伝搬の残り、carrier hold 中は媒体を完了扱いしない。

## 登録モデル、ネットワーク接続と復元

登録実装は `ethernet.SampledPhy`、モデルJSONは `dir.ethernet-phy.model` v1、workloadは `dir.ethernet-phy.workload` v1である。初期化はタイマーだけを準備し、各 callback は1サンプルのトランザクションを commit する。pause/read/export はサンプルを実行しない。出力は `dir.ethernet.PhyRecord` v1であり、TX、carrier、衝突、ready、訓練/喪失、実際の受信または失敗を記録する。`trace_samples=true` の場合は actual TX/RX µV 配列も出力する。

`phy::attachment::AttachedMedium` は network の direction ID / from_port / to_port と型付きの結合を行う。`TxSerialized`、IFG後の `TxReleased`、実際の `Received` は別の効果である。受信ヘッダの解釈が未対応の場合は `DecodeRejected`、物理的な ancestry が一意でない場合は `UnattributedReceive` とする。予定された Ethernet Arrival はこの経路の受信成立に使用しない。静穏化まで受信が無い転送は、その後に `Failed` を返す。

受信側の Ethernet II バイトだけから元のデータ/パディング境界を推定できない。復元した `EthernetWireFrame` はデータとパディングの全バイトを `data_hex` に保持し、`pad_bytes=0` とする。R-TAG の sequence/reserved は実際の受信バイトから取り出す。送信元の application length を参照して末尾0を切り捨てない。

動的ネットワーク接続はサンプル時計を連続して進める。媒体の `has_pending=false` は現在の転送と IFG/波形の静穏化を意味し、将来の admission がある時計を飛ばす許可ではない。ノイズ、ADC、タイミングループと位相の因果状態を飛ばす idle shortcut は実装していない。TAS/CBS の選択時には、このパイロットと2チップ/bitの実占有時間を用いる必要がある。

checkpoint codec は v1であり、全TX/RX/FIR/PRNG/部分フレーム、epoch、伝搬 ancestry、MAC retry/IFG、タイマーを保存する。構造/数値/不変設定と active request = queue head、pending/terminal admission の分割を検証する。登録モデルは保存された実 FES envelope の時刻・schema・payload・owner も検証する。ネットワーク接続では外部転送と PHY admission の ID/direction/実 MAC SHA-256 の全単射を追加検証する。ハッシュを受信 payload の正解判定には使用しない。

有限境界は endpoint16、admission4096、MAC長64..16384、保存する伝搬履歴4096 samples、総数値サンプル最大100,000,000である。上限到達時は実際の pending prefix を残した失敗とし、未来の受信を補完しない。主に精度と因果性のモデルであり、全設定で実時間動作できるという性能契約は無い。

## 受入境界とIEEE選択契約

本書のproject契約は独自の訓練pilotとManchester符号を持つ。IEEE選択契約は、ローカル取得済みのIEEE Std 802.3-2022本文・符号表・状態図を基に別経路で実装する。[詳細機能仕様](../specs/models/IEEE選択PHY詳細機能仕様書.md)、[設計](IEEE選択PHY詳細設計書.md)、[検証仕様](../verification/cases/IEEE選択PHY検証仕様書.md)、[参照版とhash](../verification/fixtures/ieee-phy/source-manifest.json)を正本とする。標準PDF/textを配布物へ含めない。

| 範囲 | 契約と証拠の区別 |
| --- | --- |
| P01/P02 | named PHYの実startup、role conflict、Clause98 ANはIEEE選択契約で検査する。projectの固定遅延で代替しない。 |
| P03/P04 | projectの受信訓練・Manchesterと、named PHYの再起動・mapper/scrambler/FEC/DMEを各実経路で検査する。 |
| P05 | FIR/ADC/等化/追従、32/64比較、seeded noise、chunk/checkpointは宣言した数値精度包絡の証拠である。 |
| P06/P07 | projectの共有電圧と、named PHYのClause4 collision/jam/BEB・signal repeaterを区別する。repeater chainは選択契約に含めない。 |
| P08/P09 | T1S codecと分散PLCAはIEEE選択契約の受信・Control/Data状態を使う。中央round-robinで代替しない。 |

各項目の実施結果は[実装台帳](../implementation/remaining-capabilities.json)と検証記録を参照する。

実行例は `examples/ethernet/phy/numerical.ini`、検証は `tests/phy_channel.rs` と `tests/phy_medium.rs` を参照する。
