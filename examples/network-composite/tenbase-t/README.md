# 10BASE-T point-to-pointの実sample結合

2つの実MAC portを一本の10Mb/s NED carrierへ結び、20Mchip/s Manchester、16samples/chip、宣言した50nsの対向channelを通して受信bytesを復号する開発中fixtureです。既存project数値PHYのpilot/headerは使いません。

```sh
dir-simulator run --config examples/network-composite/tenbase-t/model.ini --output tmp/tenbase-t
```

リンク起動・信号repeaterの結合と製品gateを進めています。point-to-point codecの存在だけでClause 9 repeaterの完了とは扱いません。
