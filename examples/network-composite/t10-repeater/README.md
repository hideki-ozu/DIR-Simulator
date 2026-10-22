# 10BASE-T1S multidropの実sample結合

3本のNED spokeを一つの電気的mediumへ結合する開発中fixtureです。1Gb/sのlocal linkから入ったframeを、10Mb/s MAC・4B/5B・DME・sample channel・受信PCSへ通します。hubは媒体の接続点でありL2転送ownerを持ちません。

`physical_ports`は一つのelectrical endpointを実tx/rx portへ結びます。送信は一回、受信観測はreceiverごとです。32samples/coded-bit UI、自己coupling、p0→p1は40ns・p0→p2は80nsのchannel、seeded noiseless referenceを使います。`plca.ini`は同じ媒体へnode ID 0/1/2のPLCAを結合した追加fixtureです。

```sh
dir-simulator run --config examples/network-composite/t1s-multidrop/model.ini --output tmp/t1s-multidrop
```

製品結合・保存gate中の未公開fixtureです。これだけでPHY全機能・電気適合を証明しません。

MACのFCS終端、PCS終端と媒体解放は別の実績です。受信は実sampleの復号・FCS検査後にだけ発生します。保存復元では電気的prefixを再計算し、各network ingressと受信先・時刻・bytesを照合します。管理control、CBS/TAS、wire-controlの結合は実装中で、現時点では入力時の製品制約として拒否します。
