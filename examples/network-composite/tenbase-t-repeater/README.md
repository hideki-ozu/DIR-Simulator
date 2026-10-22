# 10BASE-T repeaterの実sample結合

3つのMAC stationと3つのrepeater MAUを、3本の独立した50nsの双方向channelへ結ぶ未公開fixtureです。NEDのhubはphysical domainの接続点で、L2転送・FDBを持ちません。各MAUの受信sampleからchipを復号し、Clause 9 coreが遅延させて他MAUから再送出します。元MACから宛先MACへの直接channelはありません。

```sh
dir-simulator run --config examples/network-composite/tenbase-t-repeater/model.ini --output tmp/tenbase-t-repeater
```

canonical contractは`ieee8023.10base_t.half.2022`と`ieee8023.repeater10.2022`です。以前の`ieee8023.10base_t.2022`は同じ半二重modeへの明示入力aliasとして受理します。製品・保存gateとClause 14のlink pulse/link-loss/jabber結合を継続中です。
