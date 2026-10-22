# Local-clock TAS、CBS、GCL更新

`model.ini`は1Gb/sの実network queueに64B MACを4個入れ、clock rate3/2、送信中の相殺する二step、後退/前進step、全体時刻5usのGCL置換を実行する。管理有効化の5usとlocal base0は独立している。

独立計算では送信占有672000ps、送信credit傾き−750Mb/s、待機回復250Mb/sとなる。初回releaseの−504bitは2688000psで0になるがguard不足で開始できず、3usまでに+78bitになる。3usの前進stepでgateが閉じ、3.5usに再び開くため、二番目は3.5usに開始して−426bitでreleaseする。次の開始は5.876us、GCL置換後の閉窓を経た四番目は12.564usとなる。期待SOFは`[0,3500000,5876000,12564000]`ps。clock stepそのもののcredit面積は0である。

このfixtureは型付き構成の結合試験用で、未commit・未公開である。規格全体の適合試験ではない。
