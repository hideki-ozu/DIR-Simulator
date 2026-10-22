# 共有PHY上の制御FIFO

3つのMACが1つの10BASE-T1S数値媒体へ接続される例です。RSTPのBPDUは通常の制御FIFOから同じ物理MACへ投入され、実際の衝突・jam・再送・復号を通って各受信機へ届きます。NED上のhubは接続点を表し、L2転送を実行しません。

```sh
cargo run -p dir-simulator -- run --config examples/network-composite/t1s-control-fifo/model.ini --output /tmp/t1s-control
python3 docs/verification/fixtures/ieee-phy/verify_control_result.py /tmp/t1s-control/results.json
```

独立oracleはIEEE 802.3 length/LLC形式、zlibによるFCS、受信機別の実PHYバイト列・標本位置、およびプロトコル受信との一対一対応を検査します。予定到着時刻から受信を補いません。標準的なRSTPの秒単位の収束全体をこの300µs例だけで検証するものではありません。

制御パケットはデータのCBS/TASゲートから独立した制御FIFOの許可を用います。物理MACが送信・defer・retryを保持している間、同じ出力からデータを重複送信しません。保存・別プロセス再開は実行元からの厳密なprefix再実行とcodec18で検証します。
