# IEEE選択PHY詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`spec-ieee-phy`
文書状態：v1.2.0収録。検証範囲と実行結果は検証記録参照。

<a id="modes"></a>

```trace
{"id":"spec-ieee-phy#modes","stage":"spec","requirements":["DIR-REQ-0315","DIR-REQ-0316","DIR-REQ-0317"],"upstream":["DIR-FUNC-0092"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

既存のproject数値Manchester PHYを保持し、IEEE 802.3-2022の次の3系統を明示選択する。mode名だけの追加や固定latencyによるFEC代用は受入条件を満たさない。

| 選択mode | 規範 | 機能 |
| --- | --- | --- |
| 1000BASE-T1 | Clause97/98 | 全二重point-to-point、実訓練、InfoFieldとlink monitor、forced roleまたはBASE-T1能力交換、80B/81B・RS(450,406)・scrambler・PAM3 |
| 10BASE-T1S multidrop | Clause4/147/148 | 半二重mixing segment、4B/5B・scrambler・DME、独立MACのcollision/retry、任意のPLCA |
| 10BASE-T half duplex + repeater | Clause4/9/14 | Manchester MAU、実signal再生・jam伝搬・port partition/reconnection、単一collision domain |

1000BASE-T1はMASTER/SLAVEの受信訓練結果とcounter/countdownからdata状態へ進む。timeoutだけでlink upにしない。Clause98能力交換も実DME pageとACK/CRCから判定し、未実装rateをadvertiseしない。link loss時は不完全受信を破棄して再訓練し、管理down、PMA lock喪失、PCS/FEC errorとMAC FCS errorを区別する。

T1S multidropはauto-negotiationを使わない。PLCAのnode0がBEACONを送信し、各nodeが受信BEACON/COMMITとcarrierに従って送信機会・burst・yieldを進める。coordinator不在時は通常CSMA/CDへ戻り、独自のcoordinator選出を作らない。通常入力ではID重複を拒否し、明示した故障入力でのみ重複時のcollision/recoveryを試す。

媒体は各receiverへの遅延・反射・ノイズを持ち、3台以上の独立MACを扱う。repeaterはsignal/jamを中継し、switchのframe転送で代用しない。数値精度は宣言したchannel/receiver条件の32samples/UIと64samples/UIで比較する。電気測定・機器認証の代用とはしない。

### 起動失敗とPLCA回復

Clause97の初期maxwait満了、訓練side-stream/InfoFieldの復号失敗は永久Faultでなく、相手取得情報、訓練counter、countdown、PCS受信組立を破棄して新しいsample起点の起動を行う。AN選択時はClause98の実page/ACK交換から再開する。回復中はMAC payload送信を許可しない。役割競合などの終端AN拒否は回復待ちと区別し、失敗したadmissionだけを送信終了成功・EOF・受信を生成せず解放する。

PLCA DataはHOLD、COLLIDE、DELAY_PENDING、PENDING、WAIT_MAC、TRANSMIT、FLUSH、WAIT_IDLE、ABORTを実MACのcarrier、JAM、BEB、IFGと接続する。遅延線は99 nibble、MAC JAM終了後のpending待ちは512 MAC bit time、MAC待ちは288 MAC bit time。同期中の無効BEACONは、最後の有効BEACON終了から4000nsのtimerでRESYNCへ戻る。burstは次frameが既にqueueに存在するかで決めず、countとtimerに従って送信中または送信後windowへの到着を待つ。timeout/abortでCOMMITを停止する。

T1Sの制御取得は、実受信ADC標本の位相候補と既に届いたSYNC/BEACON/SSD群だけを参照とする。20chip以内のDME/5B同期と、設定した数値receiverの最小訓練数を分ける。最小32chipの設定では40chip受信時に数値lockを得る。未来のSSDやsenderのpayloadを訓練正本にしない。連続burst COMMITでは物理lockを保持し、JAM/信号喪失では受信組立と取得参照を無効にする。

MIIのMCD/PMCDは既存TX発振器のsample indexとclock_ppmから有理位相の交差で導く。source/output開始のニブル番号からMAC終端を計算し、正負ppmで生じるsample端点の丸めを保持する。通常のCOMMIT解除は次の群の生成を止め、既にMCDで開始した10chip群は同じqueue/driverで完了する。真の物理collisionによる割込みとは区別する。
