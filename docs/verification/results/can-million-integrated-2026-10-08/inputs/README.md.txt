# CAN 100万要求の性能測定入力

文書ID：`can-million-performance-input`

文書状態：入力生成と測定手順。実測値と未取得の量は検証記録で区別する。

`docs/品質・配布方針.md` 第1節の32送信元・単一CANバスを、乱数を使わず生成した入力である。各送信元は標準ID `0x100`〜`0x11F`、8 byte全0、31250要求、キュー容量64、処理・経路遅延0。バス速度は500000 bit/s、集計窓は既定の1 ms。

`generation.json` に、CRC-15のシフトレジスタ計算と多項式除算による照合、stuffing後の各フレーム長、3 bit間隔を含む `C_i`、整数式で求めた周期・位相・終了時刻、および全入力のSHA-256を保存した。INIの終了時刻 `T` までに各送信元が31250件を生成する。要求の送信完了件数とは別の値である。

| 負荷 rho | 周期 P (ps) | 終了 T (ps) | 生成要求数 |
| --- | ---: | ---: | ---: |
| 0.30 | 26660000000 | 833125000000000 | 1000000 |
| 0.90 | 8886666667 | 277708333343750 | 1000000 |
| 1.20 | 6665000000 | 208281250000000 | 1000000 |

入力の再生成:

```sh
python3 scripts/performance/generate_can_million.py \
  --output docs/verification/fixtures/performance/can-million
```

測定はRust 1.85.0のreleaseビルドを固定し、同時実行せず、各条件でウォームアップ1回と測定3回を行う。GNU timeで製品CLIの準備から出力確定までのwall秒と最大RSSを取得する。生成・ビルド・測定後のhash検証は計時の外に置く。

```sh
/home/hideki/.cargo/bin/cargo build --locked --release -p dir-simulator
python3 scripts/performance/measure_can_million.py \
  --binary target/release/dir-simulator \
  --inputs docs/verification/fixtures/performance/can-million \
  --output results/performance/can-million-measurement
```

測定器は各試行のstdout、stderr、GNU time、250 msごとのプロセスRSSとホスト空きメモリ、集約JSONを保存する。出力が完成した場合はmanifestの4ファイルのサイズ・SHA-256を照合し、`simulation` とrun_idを除いたCSVを3回で比較する。大きな出力・一時journalはサイズと検証結果の保存後に削除する。完成した出力を保持する場合は `--retain-outputs` を指定する。

ホスト枯渇を避けるため、既定で仮想メモリ26 GiB、RSS停止24 GiB、ホスト空きメモリ下限3 GiBを適用する。この保護条件は2 GiBの合格基準を変更しない。保護条件下で中断した試行について、無制限実行の最大RSSや完了時間は確定しない。観測RSSが2 GiBを超えた場合はメモリ目標の未達を確認できる。イベント処理だけの時間はCLIのstdoutにある `event_processing_wall_seconds` から取得し、出力確定までのwall秒と区別する。中断などでCLIの計時値が得られない試行はnullとして記録する。

2026-10-06の測定では180秒の観測上限も適用した。120秒を超えても出力未確定の試行をそこで停止し、完了時間の下限を別に保存する。同じ条件を再現する場合は、上の測定器と同時に、別のターミナルで次を実行する。この監視器は同じ測定ディレクトリの製品CLI子プロセスだけを停止する。

```sh
python3 scripts/performance/monitor_wall_limit.py \
  --measurement-root results/performance/can-million-measurement \
  --binary target/release/dir-simulator \
  --limit-seconds 180
```

2026-10-06の実測記録は [can-million-performance-2026-10-06.json](../../../results/can-million-performance-2026-10-06.json) に保存する。WSL2上の測定はnative基準機とは別系列として扱う。

2026-10-08の局所最適化と比較測定は [CAN出力の局所最適化・実測記録](../../../results/can-export-optimization-2026-10-08.md) に保存する。3,200・16,000要求の完走比較と、100万要求の60秒CPUサンプリングを記録した。最適化後の100万要求が120秒以内に完走するかは未検証である。
