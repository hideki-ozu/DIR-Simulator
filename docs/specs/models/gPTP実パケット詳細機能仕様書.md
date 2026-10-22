# gPTP実パケット詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`spec-gptp-packets`
文書状態：v1.2.0収録。実装・検証の範囲は検証記録を参照。

<a id="packets"></a>

```trace
{"id":"spec-gptp-packets#packets","stage":"spec","requirements":["DIR-REQ-0314"],"upstream":["DIR-FUNC-0091"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

compositeの`gptp.profile="as2020.domain0.two_step_tree.v1"`はIEEE 802.1AS-2020のdomain0、instance-specific P2P、two-stepの選択契約である。全二重point-to-pointのforestで、各nodeにClockIdentity、品質、PortNumberと実physical portを指定する。Pdelayと受信AnnounceからasCapable、GM、Slave/Master/Passiveを選び、実Sync/Follow_Upでapplication clockを補正する。構成されたpeerのdriftや伝搬値から同期結果を直接作らない。

PTP packetはlink-local宛先`01:80:c2:00:00:0e`とEtherType`0x88f7`で通常の有限queue、serializer、伝搬、FCS検証を通る。bridgeはそのpacketをL2で透過floodせず、自己PortIdentityを持つpacketを生成する。MAC Mergeを使う場合はexpressへ割り当てる。

時刻観測はSFD直後の最初のsymbolで行う。raw LocalClockは初期oscillatorの有理数lawを保ち、TAS/PSFPが参照するapplication NodeClockはpacketを根拠に補正する。Follow_Up受信までの経過とrelayの実queue待ちを補正し、EOF時刻をSFD観測に読み替えない。

loss中は最後のapplication affine lawでholdoverし、同期済み表示を外す。GM変更・link epoch変更は旧pairを無効化し、新しいpacketを待つ。one-step、CMLDS、複数domain、非対称delay、動的GM品質変更、Signaling interval交渉などは初期選択契約外で、具体的な入力または受信診断を残す。実装されていない他adapterとの組合せも、規格上の禁止と実装制限を区別する。


名前付きsampled PHYのcanonical MAC経路では、理想linkの`SOF+64bit` capture timerを生成しない。TXは実`TxMacSfd`、RXは完全な実MAC bytesを検証した`PhysicalIngress.rx_mapped_sfd_sample`を根拠とする。captureの`at_ps`は互換用の整数ps参照cut、追加の`sampled.reference`はattachment originとsample gridから得た正確な有理数、`sampled.observed_at_ps`はmarkerが処理可能になった現在のFES時刻である。遅れて得られるRX timestampも初期raw oscillatorで評価し、途中のservo補正を混入しない。sample/attachment/attempt/planeとpacket/link identityは保存する。理想adapterの追加field省略と算術検証は維持する。

sampled経路は全二重point-to-pointの名前付きPHYのみを許可する。半二重/shared、project numericalのMAC markerは各参加portの具体的な制限として拒否する。raw Mergeの実xMII delimiterと受信decode availabilityは異なる参照planeである。正確なtimestampはcanonicalの`MdiMacSfd`またはraw Mergeの`MdiMergeDelimiter`に限定し、実rendererのTX delimiter-endと、同じchannel historyの単一unity tapのsample delayを結合する。実受信decode/FCS/identity検証後にのみRX captureを公開する。固定clock、trackingなし、noiseless、unity impulseのTX/RX filter、clippingなしという共通PHY predicateを参加portごとに要求する。既存の再構成RX cadence座標は別fieldに保持し、物理参照へ読み替えない。対応mappingが無いprofileは具体的に拒否する。別portの物理ownerや別nodeのscript時計を一括禁止しない。同一nodeのgPTP/script時計は単一writer制約で拒否し、CBSとGCL更新は既存のclock-aware TSN ownerを使う。

sampledのPdelayはwire timestampのfractionを固定小数点へfloorする量子を明示する。量子q=125/8192はneighbor raw-clockのps単位であり、物理global psではない。直前exchangeとの4時刻から得た点推定D0に対し、alpha=(T4−T1)/(T4−previousT4)、E=q*max(1,alpha)/2の開いたwire量子化包含区間(D0−E,D0+E)を保持する。負のD0でも−E<D0<0なら非負制約に整合する推定0を選び、`quantization_limited_zero`と正確なD0/q/E/rate上下界を公開する。D0<=−Eは拒否し、rate下界は厳密に正であるという保守的marginを要求する。正のsampled推定にも包含区間を記録する。推定rate/delayの組が一つの真のhistoryに同時に一致するとは主張せず、thresholdは推定値へ適用する。固定raw oscillatorとexchange間の固定return pathが前提である。idealの従来負値拒否と保存検証は維持する。


### sampled PHY の実 readiness とプロトコル epoch

参加する sampled 1000BASE-T1 の初回両端 training 完了後は、gPTP の `link_up` を管理ポリシーと実両端 PHY readiness の積で決める。MAC busy は readiness 喪失に含めない。実 readiness 喪失を観測した同一 committed cut で link epoch を進め、asCapable、Pdelay pending／推定値、Announce candidate、選択 parent と同期状態を通常の link reset 経路で破棄する。旧 epoch の未送信 gPTP queue は退役させる。管理ポリシーを up に戻しても実 retraining が完了するまで再活性化しない。

初回 training 前は従来通り実 PHY admission に制御パケットを queue できる。最初の実 bilateral Ready を観測した後の喪失だけを上記 mask の起点とし、`physical_links` と順序付き `PhysicalReadiness` cause を checkpoint に保存する。初期 raw oscillator は回復でも変更しない。検証は idle／送信中の物理断、管理 up と実 readiness 回復の間の抑止、再学習後の packet 由来補正、および fresh-process continuation と coforged readiness epoch の拒否を対象とする。

固定receiver profileのtimestamp mappingは実training完了後のmarkerを対象とし、任意sample位相でのtraining成功を保証しない。750MBd・16samples/UI・既定固定receiverの15sample pathはPAM2 training level faultを実際に返し、gPTP captureを生成しない電気的negative gateとして保持する。0／4／16／17sampleの正例と区別する。sampled CBSのapplication結合gateでは明示的な`dir.experimental.sampled_wire_cbs.v1` accounting policyを使い、標準適合とは主張しない。


### configured full-duplex MDI SFD

configured mediaの全二重点対点ordinary MACでは、実送信Attemptに結び付く`MdiMacSfd`を用いる。64bit時間をδとし、TX参照は実MAC SOF+送信端TX latency+δ、RX参照はTX参照+伝搬である。raw timestampは各参照時刻で不変の初期oscillator lawを評価する。raw値へlatencyを直接加算しない。送信端と受信端の4個のTX/RX latencyは非ゼロかつ異なってよい。対称伝搬とbitrateは必要で、Pdelayはneighbor raw単位の伝搬のみを推定する。

受信利用可能時刻は実EOF+送信TX latency+伝搬+peer RX latencyである。RX capture自体では受信を成立させず、対応する実serialized AttemptのarrivalとFCS/packet検証後にprotocolへ解放する。capture証跡の`eof_planned_ps`と`arrival_planned_ps`は予定値であり、実EOF/受信の証跡を代替しない。configured proofとsampled proofは相互排他で、sampled/autonegotiationや完全規格適合の証拠とはしない。

TX latencyがframe+IFGより長い場合も許可する。EOFとTX captureを合流し、両者が成立した時刻にFollow_Up生成を一度だけ判断する。MAC解放後の次送信によって過去Attemptのidentityを更新しない。link/admin epoch変更で遅延captureとprotocol completionを抑止するが、物理EOFは保持する。PAUSE/TAS/CBSはSOF前のadmissionを遅らせ、既に送信中の参照点を移動しない。


Clause99 EXPRESSとproject abstract EXPRESSも、producer自身が保持する実SOF時のsource identity/timelineを消費する。`MacSource`はordinary Attempt、Merge packet ID/epoch、abstract transfer/serializer epochを区別し、proofのplaneとは独立である。Mergeのlive packetをpruneした後もproducerの`ExpressSourceEvidence`を保持する。実EOF/arrivalを予定値と別fieldに記録し、GPが受け取るEOF packet ID/tokenと検証済みbytesを照合する。configured EXPRESSは同じMDI latency則、ideal abstract EXPRESSはendpoint latency0と元linkの伝搬を持つ`IdealMacSfd`である。GP classは参加outputでEXPRESSを要求し、fragmented GPは拒否する。application delivery集計はGPのlink-local protocol frameから生成しない。

## 同時刻の管理操作とsource証跡

同時刻の`link_set`も入力順に観測し、down/upの各変化でportの世代を更新する。最終policyだけで変化の有無を判断しない。旧世代のcapture・未完EOF join・受信bodyは、新世代のFollowUpや同期交換を解放しない。実際に完了した送信・受信と歴史的source証跡は保持する。

公開captureのsource数値IDは独立したproducer記録のattempt/transfer indexに照合し、対応するstable transfer ID、epoch、direction、実SOFと物理時刻算術も検査する。この公開表現の整合検査は、private source replayと凍結入力prefixの復元検証とは区別する。
