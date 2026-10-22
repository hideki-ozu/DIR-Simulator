# IEEE選択PHY検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-ieee-phy`
文書状態：v1.2.0収録。検証範囲と実行結果は検証記録参照。

<a id="dir-test-0150"></a>

```trace
{"id":"DIR-TEST-0150","stage":"verification","requirements":["DIR-REQ-0315","DIR-REQ-0316","DIR-REQ-0317"],"upstream":["design-ieee-phy#modes"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

初期のIEEE core9件は通った。RS parityは独立Python root-equation solverのvectorと照合し、1/21/22symbol errorと選択した23symbol例を検査した。80B/81Bは1024 control-mask、45block/OAM/RS/scrambler/PAM3の変換を確認した。T1Sの4B/5B、scrambler、DMEも単体で照合した。これらはcoreの証拠であり、P01〜P09の実製品結合完了ではない。

必須製品gateはCLIからcompiler、共通FES、実codec/sample、decoded ingress、結果/Viewerまでを通す。1000BASE-T1は役割競合・能力不一致・CRC破損・restart、実訓練とdata前の抑止、correctable/uncorrectable codeword、lock lossと再訓練を確認する。22を超える全error patternの検出保証を主張しない。

T1S/PLCAは3MAC以上、tap別carrier/collision、jam/BEB、sparse ID、burst/yield、coordinator喪失復帰、ID255無効、故障ID重複を確認する。repeaterはframe cache転送を使わず、signal再生、jam先頭規則、遅延上限、port partition/reconnectionと一collision domainを検証する。

数値精度は32/64samples/UIでRMS差1%以下、edge時刻差1sample以下、noiselessで同じdecoded symbol/stateを確認する。channel impulse/gain/group delay/clippingとseeded noiseを独立に照合する。whole-run、sample batch、途中horizon、checkpoint再開を比較し、残sample、symbol、訓練/AN/PLCA timerと所有者の意味改変を拒否する。

実network結合ではT1Sの一送信・二つの独立受信・一つの実宛先受信を実行し、5つの別process保存cutと6種の改変拒否を確認した。独立checkerはVLAN10/空payloadの68BとFCS、preamble→SFD 6.4us、SFD→FCS 54.4us、40ns/80nsのtap別伝搬を照合した。PLCAの非coordinatorが40usに生成した要求は、実BEACON/COMMITを経て114.08usに二つのPHYで復号され、114.691usに宛先へ届いた。次cycleでcur_idが残る不具合の修正を含む。

10BASE-Tはpoint-to-pointと六endpoint repeaterをCLIから実行した。実decoded ingressはそれぞれ1/2件で、junctionやMAU-only endpointにL2受信を作らない。16msのnormal link pulseは5.12M個の実sampleを通し、送信の二つの正chipと1chip遅れの受信を確認し、MAC carrierやRxFrameを生まなかった。SFDとFCS markerを各送信で一つにし、preamble→SFD 6.4usとSFD→最後のFCS bit 54.4usを独立checkerで照合した。repeaterのCC31 partition、Tw5/Tw6の受信抑止と実通信復帰も確認した。

BoundaryV2の修正後binaryでは、実一送信・二受信、binding準備拒否、6種のphysical/network証拠改変、別processでの5保存cutと継続の計5製品testが通った。scalar-newとbatched-newの全record比較、noise/trace、排他的horizon、上限最後の実受信は別の追加gateで検査中である。旧wake照合失敗と初回の誤ったFCS markerの結果は破棄せず、修正後の結果と区別して保存している。

1000BASE-T1は実sampleを通したANから256個以上のInfoField、training/countdown、64B MACの連続PCS/FEC受信までのcombined gateが通った。別々のAN gateと強制MASTER/SLAVE gateも実行した。network CLI、役割競合・故障からの復帰、32sample精度、raw MAC Merge、有効なshaper結合の最終gateは実施継続中である。

実結果のViewerはT1S、遅延PLCA、T10 point-to-point/repeaterをPC/mobileで確認し、実受信前の表示抑止と逆再生を検査した。これらの実施範囲はP01〜P09全体の完了を意味しない。[独立照合手順と出典](../fixtures/ieee-phy/README.md)を参照。

### 起動失敗・PLCA回復の追加gate

`phy_ieee_gbt1`の初期静穏gateは97.5ms全sampleを実行して、2端点のmaxwait満了cut、再起動、相手復帰、新規byte/FCS復号と送受信の準備完了を別に検査する。訓練中断の再取得、AN有効のactive outage後の実Clause98再交換、終端AN拒否の即時admission解放も別gateとする。短いMDI喪失を分離して与えるdecoder拒否は、持続信号喪失によるlink再起動や管理link_setから区別する。

`phy_ieee_t1s`は実HOLD開始から396bit後の衝突/JAM、512bit待機、PENDING/WAIT_MAC、実retryと2台のbyte/FCS復号を照合する。288bit WAIT_MAC満了ではData WAIT_IDLEとControl ABORT/COMMIT停止を確認する。HOLD取消は媒体へframeを出さずData ABORTを通り、再開後の実frameはa>0のFLUSHを通る。物理COLはTRANSMITを維持し、32bit JAM中のCOL解除と遅延a=0保持、MAC終了MCDでのWAIT_IDLEを照合する。次要求は最初のframe中、または100bit経過したburst window内に投入する。3台のnamed送信機が実carrier/衝突/JAM/BEBを通ることを確認する。slot内最終sampleと直後sampleのlate faultは明示的な非対称receive-open故障で検査し、適合mixing segmentの伝搬配置と称さない。

取得gateは16/32sample/chip、両極性、全sample offset、因果FIRとseeded noiseで実復号を検査する。実ADC参照の20chip制御同期/40chip数値lockと、弱信号・clipping・参照改変の拒否も確認する。coordinator無効化/復帰では130090bitのstatus hysteresisを実sampleで待つ。

公開入力`examples/network-composite/t1s-multidrop/plca-pending.ini`は疎ID7の遅延要求を既存NetworkState/FESから実回復させる。`phy_ieee_recovery_network`は20chip取得中、内部MAC JAM中、pending中と実frame中の別process保存・再開、全結果一致、再hashしたindex/phase/polarity/count/epoch/lock/pending timer、MAC入力終端、PLCA enable、recent ADC、MAC-only JAMフラグ/countdown、SIGNAL_STATUS、遅延a、MII開始端点の15変異拒否を確認する。実行失敗を含む各logとsource/binary hashは作業evidenceのphy-recovery-completionへ保存し、最終の親review前にP01/P03/P09完了と扱わない。

追加のnetwork burst gateは、実一frame実行から上流MAC→PHY admission遅延とcoded endを測り、2回目のアプリケーション要求を最初の送信中、またはcoded end後100bitでMACに到達させる。実2回目SOFが128bitのburst window内であり、その間にBEACONがなく、4件の実RxFrameがあることを検査する。

HOLD強制衝突のJAMはMAC側だけで生成する。coordinator BEACONと内部JAMを重ね、要求なしのbaselineと全実MDI送受信標本を比較する。32bit JAM、BEACON継続、偽のframe marker/originがないことと途中checkpoint継続を確認する。3台の実物理衝突による媒体JAMは別gateで維持する。

pending回復の新しいPCS効果について、baselineで得たDELAY_PENDINGの実sampleを最終budgetに置く。次sample境界の排他的horizonと、その1ps後の失敗runを比較し、最終有効効果の保存と失敗処理で追加効果がcommitされないことを検査する。

MII/PCS接続は既存TX発振器のsample indexとclock_ppmから有理位相の交差を導く。ゼロppmではPMCD phase120から40sample後のMCD phase0でBEACONを駆動し、実波形から5群と2chipの終端zeroを照合する。正負ppmではsource/output両開始のニブル番号からMAC入力/出力終端を独立算出し、開始時のsample遅延加算では1sampleずれる例を含める。

COMMIT timeoutの最終群はゼロ・正負ppmの実電圧標本で独立DME照合し、10chipの完了、通常chip cadence、余分な群がないこととWAIT_MACの有界性を検査する。解除直後の残群途中でcheckpointを保存し、同じ波形への継続と不正な残群状態の拒否を確認する。対応試験は`actual_commit_timeout_drains_sampled_group_and_checkpoint_at_zero_and_nonzero_ppm`。
