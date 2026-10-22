# MAC Control PAUSEの独立照合

文書ID：`verification-mac-control-fixtures`

`verify_result.py`はシミュレータのcodecをimportせず、実結果のPAUSE frameとtimerを検査する。Python標準ライブラリの`zlib.crc32`でFCSを計算し、予約multicast DA、EtherType、opcode、quantaのbyte順序とzero paddingを照合する。送信と実受信のID・bytes、および実受信要求からタイマーをloadした時刻と物理rateによるexpiryを対応付ける。

```sh
python3 docs/verification/fixtures/mac-control/verify_result.py /tmp/pause-media-run/results.json
```

対象は[公開例](../../../../examples/network-composite/pause-media/README.md)などの、実際に生成・受信した標準multicast PAUSEである。少なくとも一つの正のquantaによるtimer loadを要求する。任意のunicast raw client、電気波形の破損、CBS解析積分、全IEEE適合の検査はこのcheckerの対象に含めない。
