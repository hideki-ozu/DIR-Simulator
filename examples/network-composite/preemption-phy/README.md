# 出力ごとのproject preemptionと共有PHY

`Net.a.tx`だけが既存のabstract preemptionモデルを使用し、後段の3つのMACは10BASE-T1S数値媒体を使用します。元の68バイトMACは20・20・20・8バイトのproject fragmentとして前段を通り、後段では同じ元MACの実波形を復号します。

```sh
cargo run -p dir-simulator -- run --config examples/network-composite/preemption-phy/model.ini --output /tmp/preemption-phy
```

`ethernet.network.composite.v1`では`preemption.outputs`が出力所有者を指定します。指定のない出力は通常MACを使用します。従来の`ethernet.preemption.abstract.v1`では全出力指定の入力契約を保ちます。同じ出力へのabstract preemptionとClause99 MAC Mergeの二重指定、およびabstract fragmentを直接数値PHYへ投入する指定は準備時に拒否します。

abstract fragmentの設定値はprojectモデルであり、IEEE MAC MergeのmPacket符号化とは別の契約です。この例は2つの異なる出力所有者の接続を検証します。
