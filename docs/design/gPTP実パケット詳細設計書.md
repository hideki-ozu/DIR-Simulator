# gPTP実パケット詳細設計書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
文書ID：`design-gptp-packets`
文書状態：v1.2.0収録。実packetの統合・製品gate中。

<a id="packets"></a>

```trace
{"id":"design-gptp-packets#packets","stage":"design","requirements":["DIR-REQ-0314"],"upstream":["architecture#gptp-packets"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

`runtime/network/gptp/`はcodec、checked rational math、port/node state、actual capture、receive、checkpointを分担する。NetworkState/Deltaの一つのownerで内部Frame/Transferを通常queueへ投入する。TX/RXのSFD capture tokenは実packet、direction、link epochを持ち、RXは完全受信/FCS検証後にだけprotocolへ解放する。

headerのminorVersionPTP/versionPTP送信値は`0x12`、AS2020のcontrolFieldは0である。timestampは48bit secondsと32bit nanoseconds、correctionFieldはsignedの2^-16nsである。Follow_Upの累積rateは2^-41単位で量子化し、負値もfloorする。wire境界以外では有理数を保つ。

Pdelayの4時刻と連続exchangeからneighborRateRatioを求め、meanLinkDelayはneighborのtimebaseで保持する。Sync/Follow_Upの実pairからGM/local比と受信時のGM時刻を導き、Follow_Up処理時刻までraw経過を外挿する。relayは元GM timestampを保ち、実下流SFDまでのresidenceをcorrectionへ加える。application NodeClockは単一操作でreanchor/rate更新し、local gate wakeを再計画する。

private checkpointとouter codecはversion14を選ぶ。保存した補正journalはactual Sync/Follow_UpとPdelay exchangeの証拠に結合し、値だけを信用せず再計算して最終NodeClock/generationを比較する。capture、pair、parent、sequence、timer generationとFES予約を照合する。featureが無い旧入力・保存形式へgPTP状態を追加しない。


sampled captureはidealの`Event::Capture`とは別の`CauseKind::SampledCapture`として観測時刻順に記録する。共通helperはtransfer/kind/attachment/reference sample/physical attempt/packet generation/reference planeを受け、`grid.time(sample)+origin`を有理数のままraw oscillatorへ渡す。参照cutが現在より未来なら拒否し、過去へのcallback登録は行わない。RX captureとprotocol ingressは実decode/FCS検証後の同じNetworkDeltaで順に適用する。失敗attemptの暫定captureは次attempt開始時に破棄し、成功EOFに対応したTX captureだけをFollow_Upへ使う。

sampled/gPTPのcheckpointは凍結したprepared入力からfresh Engineを保存callback数まで実行し、backend全体・journal・FES順序を照合してから復元ownerを採用する。sampled markerを保存値から逆算する第二の検証器は設けない。ideal専用のSOF/SFD算術とprivate replayの契約は保持し、sampled causeをそちらへ渡すと明示的に拒否する。raw xMII、MDI systematic symbol、decode availabilityは同じplaneとして扱わず、未証明のPHY delay補償を加えない。

正確なMDI timestampの共有可否判定は`MediumConfig::exact_mdi_timestamp_mapping_supported`が所有する。gPTP parserとcapture helperは同じ判定を使う。RXの`rx_mapped_sfd_sample`またはraw `delimiter_sample`は実TX marker、source attempt/generation、当該unity pathのdelayから得る。再構成`rx_sfd_sample`/`reconstructed_delimiter_sample`とdecode availabilityは併存させる。gPTP内で伝搬値を足してtimestampを補償せず、PHYが発行したsource-bound証拠だけを消費する。


### sampled PHY の実 readiness とプロトコル epoch

参加する sampled 1000BASE-T1 の初回両端 training 完了後は、gPTP の `link_up` を管理ポリシーと実両端 PHY readiness の積で決める。MAC busy は readiness 喪失に含めない。実 readiness 喪失を観測した同一 committed cut で link epoch を進め、asCapable、Pdelay pending／推定値、Announce candidate、選択 parent と同期状態を通常の link reset 経路で破棄する。旧 epoch の未送信 gPTP queue は退役させる。管理ポリシーを up に戻しても実 retraining が完了するまで再活性化しない。

初回 training 前は従来通り実 PHY admission に制御パケットを queue できる。最初の実 bilateral Ready を観測した後の喪失だけを上記 mask の起点とし、`physical_links` と順序付き `PhysicalReadiness` cause を checkpoint に保存する。初期 raw oscillator は回復でも変更しない。検証は idle／送信中の物理断、管理 up と実 readiness 回復の間の抑止、再学習後の packet 由来補正、および fresh-process continuation と coforged readiness epoch の拒否を対象とする。

固定receiver profileのtimestamp mappingは実training完了後のmarkerを対象とし、任意sample位相でのtraining成功を保証しない。750MBd・16samples/UI・既定固定receiverの15sample pathはPAM2 training level faultを実際に返し、gPTP captureを生成しない電気的negative gateとして保持する。0／4／16／17sampleの正例と区別する。sampled CBSのapplication結合gateでは明示的な`dir.experimental.sampled_wire_cbs.v1` accounting policyを使い、標準適合とは主張しない。


### configured MDI captureと完了合流

configured ordinary MACのSOF callbackで`ConfiguredAttempt`を固定する。index、Data transfer owner、attempt number、direction、physical link/sideを保存し、capture時には保持された過去Attemptを検証する。現在のqueue ownerや最新Attemptから引き直さない。policy/physical epochとattempt numberは別identityである。

`configured_gptp_proof`は凍結configと当該Attemptからchecked arithmeticでMDI/peer SOF、参照、予定EOF/arrivalを導く。optional `Capture.configured`にplane、参照/観測時刻、config latencyとowner証跡を保持する。理想captureとsampledの量子化算術は維持する。

`gptp_tx_eof`は物理EOFを先に記録し、共通`gptp_try_complete_tx`を呼ぶ。TX capture後も同じhelperを呼び、EOF/captureの両方が揃ったとき`tx_completed_at_ps`と順序付き`TxComplete` causeを一度だけ保存する。旧epoch/失効role/relayではFollow_Upを生成しない。保存検証はcaptureとpending reservationに同じtimeline helperを使い、両方の順序と同時刻を受理する。private replayのEOF-before-capture禁止は除き、明示的completionを再構成する。外側Engineの凍結入力prefix再実行は引き続き独立したsource bindingである。


Clause99 EXPRESSとproject abstract EXPRESSも、producer自身が保持する実SOF時のsource identity/timelineを消費する。`MacSource`はordinary Attempt、Merge packet ID/epoch、abstract transfer/serializer epochを区別し、proofのplaneとは独立である。Mergeのlive packetをpruneした後もproducerの`ExpressSourceEvidence`を保持する。実EOF/arrivalを予定値と別fieldに記録し、GPが受け取るEOF packet ID/tokenと検証済みbytesを照合する。configured EXPRESSは同じMDI latency則、ideal abstract EXPRESSはendpoint latency0と元linkの伝搬を持つ`IdealMacSfd`である。GP classは参加outputでEXPRESSを要求し、fragmented GPは拒否する。application delivery集計はGPのlink-local protocol frameから生成しない。


private保存検証では、保存時点のport epochを過去arrivalへ適用しない。順序付きLink/EOF/RX capture/Receive causeから、その実Receive callback時のprotocol許可条件を再構成する。旧epochで到着した実MAC bytesはFCS/identity証跡として残るがdecoded protocol bodyは解放しない。fault前に受理したbodyは後のepoch変更でも正当である。受信cause無しのbytes、bytes無しの受信cause、受理済みbodyの欠落、偽Link causeで作ったstale dispositionは拒否する。後段の独立private protocol replayと凍結入力Engine prefixは維持する。

## 順序付きsource LinkSet

`gptp_change_link`は既存projection同期と明示source操作の共通状態更新を担当する。`gptp_source_link`は入力順の各up/downを受け、同時刻ABAでもLink cause・世代・selectionを更新する。raw physical readinessは独立した前提として保持する。downでは旧世代のqueued protocol frameも退役させ、歴史的producer sourceと既完了capture/EOFは削除しない。

公開GP proofの内部数値indexはproducerが出した`configured_media.attempt_index`、`mac_merge.packet.transfer_index`、`preemption.decision.active_transfer_index`へ結合する。これらは既存stable source IDとの対応を補う機械可読の証跡であり、UIの操作項目ではない。legacyの公開記録では追加keyを省略できるが、新しいGP proofでは対応するsource indexを必須とする。
