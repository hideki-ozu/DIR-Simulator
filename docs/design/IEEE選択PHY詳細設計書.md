# IEEE選択PHY詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-ieee-phy`
文書状態：未公開。検証範囲と実行結果は検証記録参照。

<a id="modes"></a>

```trace
{"id":"design-ieee-phy#modes","stage":"design","requirements":["DIR-REQ-0315","DIR-REQ-0316","DIR-REQ-0317"],"upstream":["architecture#ieee-phy"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 連続PCS、実sample、shared MACと共通FESの結合を追加 |

既存`runtime/ethernet/phy`のchannel、sample grid、receiverとAttachedMediumのseamを拡張し、NetworkStateの物理ownerを増やさない。mode省略はproject数値PHYの入力・保存動作を保持する。GMII/PCS、MAC、PHY frame、physical attempt、receiver deliveryを区別し、shared媒体の一送信を宛先数だけ再送信しない。

1000BASE-T1の45個の80B/81B blockと9bit OAMから3654bitを作り、GF(2^9)のRS(450,406)で4050bitへ変換する。44parity symbol、訂正半径22symbolを扱う。role別scramblerと3B2Tを経て2700 PAM3 symbolを送る。scramblerとcodeword境界はMAC frameごとに初期化しない。

T1Sはnibble scrambling、4B/5B、80ns coded-bitのDMEを通る。half-bit chipは25Mchip/sとし、16samples/chipは32samples/coded-bit UIに相当する。idle、SSD/ESD、BEACON/COMMIT、high impedanceをdataと分ける。受信byteは受信sampleから復号してFCSを確認する。元sender payloadを受信結果の正本にしない。

半二重MACは各endpointでcarrier、collision、jam、attempt、RNGを所有する。BEBはjam後のwall-time遅延と通常のcarrier deferenceであり、idle slotだけを減らす方式にしない。repeaterのjamとMACのjamを区別する。PLCAは中央の予約配分でなく各nodeの受信state machineとして動作する。

sampleを内部batch処理する場合も、外部control、観測horizon、最初の可視effectで分割する。batchサイズ変更でfilter、noise PRNG、lock判断、時刻丸めを変えない。MAC preamble/SFD/FCS、coded-block完了、媒体releaseを別effectにし、gPTPとshaperへactual timingを渡す。

named modeのnetwork結合はBoundaryV2を使う。`lookahead_samples`は省略時4096、明示値は1〜65536で、scalar参照も同じ新しい時刻規則を使う。phase0管理controlでは現在時刻より前のsampleだけを実行し、phase1 callbackとarbitrationでは現在時刻までを先に実行する。一度処理したsampleは後の同時刻deltaでも実行し直さない。旧adapterの偶発的なevent ID順と同じであるとは主張しない。

次の可視effect又はlookahead上限はattachmentのcloneで予測し、そのsample完了時刻を共通FESに予約する。予測によって本体、乱数、記録連番やFES IDを進めない。途中の入力では経過した静穏prefixだけを本体へ適用し、入力後に残りの予測を破棄して予約を作り直す。traceを有効にした場合はsampleごとの実µV配列も可視effectになる。実行horizonは排他的であり、horizonと等しい完了sampleを実行しない。sample上限の最後の正常effectをcommitした後、次のsample時刻に上限診断を予約する。

checkpointはpartial codeword、scrambler、訓練/AN timer、channel delay line、sample phase、receiver filter/clock、各MAC retry、repeater port、PLCA node stateを含む。保存値とFES予約の所有・epoch・実送受信証拠を照合し、同時到着をendpoint反復順に解決しない。

named networkのprivate/outer codecは17である。保存したauthoritative sample cursorがcommit時刻を越えないことに加え、復元時はbundleに保存された入力からEngineを初期化し、保存されたcallback数だけ同じFES規則で実行する。導出した全network state、pending予約・ID・同時刻key、journalを照合してから、その実行によって作ったownerを採用する。保存counterを単独の正本にせず、改変したcounterに対応する結果全体も一致する必要がある。再実行は準備入力のmax_events、PHY sample上限、checkpointサイズ上限に従い、外部出力を公開しない。これは署名や入力自体の真正性を証明する機能ではない。

採用した版・clause・option・aliasと独立referenceを[出典manifest](../verification/fixtures/ieee-phy/source-manifest.json)へ記録する。`ieee8023.10base_t.half.2022`がcanonicalで、`ieee8023.10base_t.2022`は同じ半二重契約の入力aliasである。6endpoint repeaterでは3つのMAC↔MAU pairだけがchannelを持ち、アプリケーションに接続するphysical portはMAC側の3つに限る。sampleに付いた元MAC provenanceをretimeとMAU出力に保存する。

Clause14はPowerOnでLink Test Passを選ぶ許容optionを使い、通常運転中の実normal link pulse、link-loss、jabber/unjabを別に実装する。16ms pulseと100ms loss、40ms jabberと500ms unjabの選択timerを保持する。sampleの内部loopだけで未来stateを現在callbackへcommitしない。静穏区間の省略にはownerごとの厳密な零入力発展と、現在のFES cutまでの同期が必要である。

### 境界予測の再利用と失敗時刻

`AttachedMedium::forecast_until_first_effect` は状態のcloneを進め、最初の観測または有界なquiet prefixを返す。Network ownerは起点cursorと終点state/effectsを非直列化cacheとして保持し、起点が同じで予定したcutまで実時間が到達した場合にだけ採用する。途中cutの同期は元の状態から厳密に進め、全PHY変更操作はcacheを破棄する。cacheはcheckpointの状態・同値性・乱数消費・FES識別子に含まれない。将来の予測でエラーになった場合、その場で実行を失敗させず、一標本の実行予約へ戻して実際の失敗cutで報告する。

制御FIFOの通常MAC送信も同じAttachedMediumへ投入し、実際の送信開始・MAC FCS終端・MAC解放と復号済み受信からのみ進める。共有PHYでは各受信機の実際の復号を個別に配信し、予定したL2到着を生成しない。IEEE 802.3 length/LLC形式とEthernet-II形式の双方をバイト列とFCSに結び付ける。

### 起動回復と受信済み制御参照

`t1_link::Event::Restarting`はmaxwait/訓練復号失敗の回復通知であり、終端`Fault`から分ける。`Port::restart`は新しい物理sample cutにtimerを置き、相手のInfoField、seed、training/countdownと受信PCSを再取得する。generic Receiverの絶対cursor、frontend、連続sampling clockを再生成しない。終端`RetryExhausted`ではAttachedMediumが該当transfer IDだけをserialized failure→released failureとし、将来の`TxReady(true)`に依存させない。

PLCA ControlのpacketPendingはData状態から作る。HOLDの上限でsignal errorを実MACへ渡し、同じ既存Programが32bitのMAC JAMを消費する。Data COLLIDEではheld payload/JAMをPHYへ渡さず、既存PHY driverはBEACON/COMMIT/idleを独立に保持する。物理送信開始・coded end・MAC frame marker・physical originを内部JAMから作らない。PHYへ実送信中の物理衝突では従来の媒体上JAMを使う。既存のBEB counterはwall-timeで進め、Dataの512bit pending待ちとcarrier保持を独立に扱う。PENDINGの機会取得でCOMMITし、WAIT_MACでcarrierを下げてMACのIFG/retryを待つ。288bit内にMACが戻らなければDataはWAIT_IDLEへ戻り、ControlがABORTしてCOMMITを止める。物理COL中のDataはTRANSMITを維持し、遅延aを0に保持する。MAC終了後のMCDでWAIT_IDLEへ戻る。非衝突のa>0だけ実際のheld tailをFLUSHし、source/output開始のニブル番号とframe長から求めた出力MAC終端のMCDでRS出力を止める。PCSは同じProgramでESDを独立して完了する。衝突のlate判定は検出cutを保存し、JAM完了時刻で再分類しない。

T1S取得はReceiver内に最大64chip分のpost-FIR/ADC位相標本を保存する。privateなReceivedControlReferenceはepoch、chip ordinal、sample index、位相、制御群、開始/終了極性を結び付ける。20chipの制御prefixをDMEで検査し、振幅・ADC clipping・RMS残差を満たしてから制御を公開する。数値lockは最小訓練数まで既受信制御/SSD参照を追加し、候補equalizerへ時系列順に再適用する。frontendや物理時刻は再実行しない。数値lock取得の通知時刻は検査した現在cutであり、過去へ遡らせない。通常のpacket DME/PCS ownerは既存のまま、受信済みprefixを一度だけ引き継ぐ。

保存した制御参照はADC履歴から再構成し、位相、極性、count、epoch、lock、学習係数/誤差履歴を照合する。外側のnetwork checkpointは従来どおりfrozen source prefixの再実行結果と全状態を比較する。局所的な数値検査だけで、調整済みtimerやFES状態を正当化しない。

burst COMMITは自端点が保持する機会であり、MACの外部carrier扱いにしない。実Programが終了してMAC IFGを満たした時点で`TxReady`とgetterを一致させ、前frameのnetwork reservationを解放する。次frameは同じ既存queue/admissionから投入され、残りのburst window内で送信できる。HOLD/PENDING、未完了Program、外部carrierによる抑止は維持する。PHYへ事前に複数frameをqueueする試験だけで、このnetwork結合の証拠にしない。

PLCAのMCD/PMCDは既存Transmitterのsample cursorと設定clock_ppmから計算する読取りviewであり、位相accumulatorを追加しない。source MACの新しいnibble受入れ、BEACON/COMMITの最初の群とdata開始をこの境界へ合わせる。群内とSILENCEは従来driverで連続出力し、境界待ち中もidle zeroを描画する。非ゼロppmのMAC終端はsource/output開始のニブル番号とframe長から計算し、固定sample遅延の加算でFLUSHを1ニブル延ばさない。

COMMIT commandがMCD間で解除されても、開始済み群の残chipを既存`plca_control_chips`から連続出力する。将来の群を生成せず、残chip排出中はcommand値に関係なく新しいProgram開始を抑止する。checkpointは通常の解除後に残る有界な群suffixも検査する。物理collisionが要求する中断処理は別の経路として保持する。
