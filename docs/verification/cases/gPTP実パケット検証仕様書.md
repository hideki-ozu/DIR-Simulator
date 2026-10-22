# gPTP実パケット検証仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`verification-gptp-packets`
文書状態：v1.2.0収録。実packetの統合・製品gate中。

<a id="dir-test-0149"></a>

```trace
{"id":"DIR-TEST-0149","stage":"verification","requirements":["DIR-REQ-0314"],"upstream":["design-gptp-packets#packets"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

規範本文と設計の照合、実runtime hook、codec/math、parser、checkpoint version14までを結合し、製品gateを実施中である。設計書やcompile成功をT02完了としない。

独立literalはSync/Follow_Up/Pdelay/Announceのoffset、`0x12`、2.5ns correction、負のfractional rate、sequence wrap、paddingとmessageLength、無効timestamp/TLVを検査する。Pdelayの独立計算はraw rate1と1.0001、delay100ns、turnaround300nsでneighbor delay100.01ns/local delay100nsを照合する。relayの独立計算は実queue待ちを含め、二重delay加算を検出する。

製品gateはGM→relay→listenerの異なるoffset/drift、実Pdelay二回以上、実Announce選出、queue競合によるresidence変化、packet起因の時計収束を扱う。同じdata workloadでTAS SOFとPSFP受理が同期有無によって変わり、その因果packet IDまで確認する。

loss/bad FCS/遅いFollow_Up/identity不一致/重複response/link reset/GM撤退と再選出を実経路で確認する。SFD前後、RX capture後EOF前、Sync/Pdelay pair待ち、timer境界、servo直後で別process保存継続を比較する。timestamp/rate/delay/parent/targetと予約をrehashして意味改変を拒否する。Viewerのprefixは予定受信や未来の補正を表示せず、holdoverを同期済みにしない。


名前付きsampled PHYの追加gateは`network_gptp::sampled_binding`で実行する。750MBd/16 samples/UI、実Clause97 startup、非整数psのsample grid、対称channel経路を使い、実Pdelay/Announce/Sync/Follow_Up ingressとservoを確認する。各captureのraw値を初期offset/rateと正確な参照sampleから独立に計算し、RXの参照とdecode観測の差、servo後もraw lawが変わらないことを検証する。長時間実行testは明示的に`--ignored`で選ぶ。compileやadmissionだけをこのgateの合格としない。

sampled保存gateは実TX marker後・RX decode前のbundleを別processで復元継続し、同じ実ownerの継続結果と比較する。capture sample/reference/raw、observed timeとcause、attempt、plane、cause順序を外側hashも更新して改変し、fresh Engine prefixとの差で拒否する。追加fieldの無いideal capture出力と既存ideal gateも保持する。CBS/GCL更新はpacket servoと実data送出を含む独立正例で確認する。canonical/rawの対応MDI参照mappingは独立gateで確認し、0/4/16/17 samplesのdelayに対して実RX−TXが正確なsample差を保つことを検査する。4 samplesは1000/3psである。再構成cadenceとdecode観測は別座標として検証し、unsupported FIR/noise/tracking profileの明示的拒否も確認する。

wire量子化のliteral gateはalpha=0,1/68,1,2、−Eの開いた端点、区間内/外、正の点推定維持、rate下界0の保守的拒否、無効intervalとideal負値拒否を検査する。actual zero-delay canonical/raw packetの時計補正まで確認し、量子化provenanceの改変/省略を別processのfresh prefixで拒否する。sampled application gateはclass7の実GPとclass0のTAS/CBS、実decode時PSFP、GCL replacementを同時に実行し、各data判定を最新packet-servo時計の値で照合する。初期raw時計のままならPSFPの判断が変わる実data点も必須とする。


### sampled PHY の実 readiness とプロトコル epoch

参加する sampled 1000BASE-T1 の初回両端 training 完了後は、gPTP の `link_up` を管理ポリシーと実両端 PHY readiness の積で決める。MAC busy は readiness 喪失に含めない。実 readiness 喪失を観測した同一 committed cut で link epoch を進め、asCapable、Pdelay pending／推定値、Announce candidate、選択 parent と同期状態を通常の link reset 経路で破棄する。旧 epoch の未送信 gPTP queue は退役させる。管理ポリシーを up に戻しても実 retraining が完了するまで再活性化しない。

初回 training 前は従来通り実 PHY admission に制御パケットを queue できる。最初の実 bilateral Ready を観測した後の喪失だけを上記 mask の起点とし、`physical_links` と順序付き `PhysicalReadiness` cause を checkpoint に保存する。初期 raw oscillator は回復でも変更しない。検証は idle／送信中の物理断、管理 up と実 readiness 回復の間の抑止、再学習後の packet 由来補正、および fresh-process continuation と coforged readiness epoch の拒否を対象とする。

固定receiver profileのtimestamp mappingは実training完了後のmarkerを対象とし、任意sample位相でのtraining成功を保証しない。750MBd・16samples/UI・既定固定receiverの15sample pathはPAM2 training level faultを実際に返し、gPTP captureを生成しない電気的negative gateとして保持する。0／4／16／17sampleの正例と区別する。sampled CBSのapplication結合gateでは明示的な`dir.experimental.sampled_wire_cbs.v1` accounting policyを使い、標準適合とは主張しない。


### configured MDI product gates

`network_gptp_configured`は4種の非ゼロendpoint latencyと独立したphysical/raw oscillator/Pdelay oracleを使う。参照時刻とraw値はproduction helperを呼ばず、fixtureの1Gb/s・100ns伝搬と初期phase/rateから導く。frame+IFG超のTX latency、MAC EOFより前/後のRX参照、大きなRX latency、EOF/capture同時刻、MAC解放後のcaptureを対象とする。

fresh-process保存はcapture前、EOF後/capture前、capture後/arrival前、およびSync Follow_Up合流前後を対象とする。actual attempt/number/direction、reference、config latency、plane、epoch、completion、event予約をouter hashと共に改変して拒否する。実reservation failureでAttempt/capture/Follow_Up/cause/records/FESが部分commitされないこと、link fault/restartとPAUSE/閉gateのretention、実収束後のTAS/PSFP/CBSを別gateで確認する。既存idealとsampledのgate成功をconfigured受入の代用にしない。


3種ownerのreservation failureは実Engine callbackのadmission/source保持、capture、Follow_Up生成の計9経路で検査する。backend全体、記録、cause、全FES work/順序key、dirty予約とnext sequenceを比較する。callback前の既存dirty→arbitration sealingは同じ論理FESへ正規化し、失敗callbackの予約や記録を除外して比較しない。

強いgate閉鎖gateは、実packet servoが送信中のconfigured MAC frameのclass gateを閉じる条件を作る。旧raw calendarでopen、新packet時計でclosedを独立に計算し、実SOF < 実servo < 実EOF、同一cutのgate_overrun、frameの全長完送を要求する。既知の将来GCL閉鎖はfullfitでSOFを防ぐため、この検査の代替にしない。別gateでMAC解放後・MDI capture前の既知gate閉鎖も扱う。

fresh-process継続はordinaryの5cut、Merge/abstractの遅延EOF/capture両側4cut、通常source退役後2cutの計11比較を行う。outer hash更新付き21改変を拒否する。private native restoreは、fault前の受理bodyとfault後のstale bytesを同じ保存状態で検証し、受理body欠落と偽stale causeも拒否する。

## Source世代と公開数値IDの追加検証

source LinkSetのsame-cut down/up、繰返しdown、EOF/capture/arrivalを跨ぐ遅延を、configured ordinaryとabstract EXPRESSの実packetで検証する。新世代の収束だけでなく、旧世代からFollowUp又は受信bodyが生成されないこと、checkpoint継続とsource cause mutant拒否を確認する。native checkpointの正例にはsame-cut down/upの前後で受理済みbodyとstale受信bytesが共存するcutを含める。

公開proofの検証unitは、実configured ordinary・Merge EXPRESS・abstract EXPRESS・ideal abstractのcaptureを作り、数値attempt/transfer identityだけを改変したcaseを拒否する。既存のplane、endpoint、参照時刻、latency、proof排他mutantを維持する。最終実行結果は製品gateの記録で示す。
