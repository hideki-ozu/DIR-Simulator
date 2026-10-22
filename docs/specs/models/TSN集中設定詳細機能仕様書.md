# TSN集中設定詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-qcc-cnc`
文書状態：未commit・未公開。本契約のfocused製品gate・独立レビュー済み。

<a id="configuration"></a>

```trace
{"id":"spec-qcc-cnc#configuration","stage":"spec","requirements":["DIR-REQ-0302"],"upstream":["DIR-FUNC-0083"],"state":"confirmed","pending":[]}
```


### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 集中設定・受入れ判定・実ゲート適用と継続契約を追加 |

## 機能と版

`ethernet.network.composite.v1`の任意の`qcc`設定で、fully centralizedのCUC/CNCを構成する。管理APIはプロジェクト定義の型付きローカルAPI `dir.cuc-cnc.local.v1`。IEEE 802.1Q-2022 clause 46の要求・結果を扱う範囲を明記し、UNIのwire encodingや外部機器との相互運用を主張しない。

CUCはgenerator、listener、固定egress経路、合理数Interval、interval当たり最大frame数、client payload最大byte数、MaxLatency、送信offset範囲、jitter、rank、tree数を指定する。時刻とversionはdecimal stringを用いる。IntervalはCNCのcycleと一致する。NumSeamlessTrees=0は1tree、MaxLatency=0は初回のbound発見要求として扱う。rankは有限探索の決定順にだけ使用し、既存streamを暗黙に追い出さない。

## 受入れと実送信

固定経路の各hopで、実link bitrate、伝搬、送受信処理、VLAN membershipと実tagging、ingressのPVID/priority、listener MAC、queue frame/byte容量を検証する。MAC byte上限は現在のserializerに合わせてheader 14、C-tag 4、`max(client payload,46)`、FCS 4を含む。各frameのpreamble/SFDとIFGを含めて整数psへ個別に切り上げ、burst全体を一度だけ丸めない。

各managed direction/classに保守的な送信windowを配置し、前hopのwindow終端・伝搬・処理が次hopのwindowに間に合うことを確認する。最終boundはinterval原点から受信処理完了までを含み、同じcycle内にdrainする。生成した経路とTAS scheduleを実runtimeへ適用し、記録だけの受入れにはしない。managed classは割当window外、撤去後、初回設定前に閉じる。背景classは補集合のwindowを使い、既存のIFG込みguardを適用する。

sourceの契約判定は実Ready時刻で一度行う。cycle当たりのframe数、payload、割当source offsetまでの準備完了を検証し、超過・oversize・遅延・未承認sourceをmanaged queueへ入れない。generation、Ready、最初の実SOF、interval原点を区別する。背景frameのenqueueでも実際に未使用managed buffer容量を予約し、契約だけの空き容量にしない。

## 管理操作

Prepareは全active setの候補を生成し、変更前後の参加ポートからACKを集める。参加者はAccept/Reject/Silentを指定できる。ACKはtimeoutより厳密に早く、timeoutはactivationより厳密に早い。候補は同時に一つ。別Prepareはbusy、明示Cancelは対象transactionだけを中止する。

activationではversion/実topology epoch/全ACK/実queue・送信・伝搬・受信処理のdrainを再確認する。失敗は旧version・旧route・旧scheduleを維持する。rollbackは直前stream setを現在topologyで再計算し、同じACK/drainを通る。link/VLAN/STP/PAUSE等が仮定を壊した場合は保証をsuspendし、候補を破棄して新規managed sourceを止め、再承認を要する。

## 対応範囲

有限plannerは固定egress経路とdirection/classごとの有限契約を、型付きphysical/schedule/clock/producer bindingに照合する。固定affine clockの共同calendar、保守的なCBS service、検証済みFRER/provider経路、Merge control envelope、制限されたsampled経路は個々の証明adapterが成立する要求を扱う。外部schedule writer、未証明PSFP serviceや追加control producerを黙って合成しない。未対応の組合せは要求ごとに理由を明示して拒否し、実graphの実行を維持する。N13は継続中であり、これらを一般合成完了とは扱わない。

全ての探索失敗を物理的な不可能性にしない。`search_limit`、`no_schedule_in_supported_model`、`unsupported_shared_class`、`unsupported_dependency_cycle`と、単一frameがcycleを超える必要条件違反を区別する。

## 規格参照

IEEE 802.1Q-2022 46.1.3.3（CUC/CNC、printed 1667–1668 / PDF 1668–1669）、46.2.3.5（TSpec/offset/jitter、1680–1682 / PDF 1681–1683）、46.2.3.6（listener/MaxLatency/tree、1682–1684 / PDF 1683–1685）、Annex U.2（例示workflow、2148–2150 / PDF 2149–2151）。確認した全文PDFのSHA-256は`1653580bcae81ed4753ff2840b40d67239036078ed550cff23325d6fdd697114`。

## raw 1000BASE-T1 / Clause99 の証明付き合成

`examples/network-composite/qcc-raw/model.ini`は実sampled startup、両方向の実LLDP capability、Verify/Respond受信を経てからQccを準備・適用する。対象は固定clock、trackingなし、無雑音・無echo、unity single-tapの全二重2端点raw経路で、`MediumConfig::raw_timing_envelope`が成立する設定に限る。その他の経路にも実行機能はあるが、この損失なし保証は付与しない。

静的予約にはterminator→decode上限 `4080ns + Pmax + Q` を使う。`Pmax`は設定のpath delayと1UI未満の丸めから得る将来上限であり、観測offsetを上限として採用しない。callback roundingは本profileで0、共通FESの整数ps切上げを別途1ps予約する。RS drainは伝搬・decode遅延にだけ含み、serializerの占有へ加算しない。

実際のserializer admissionを数え、`L(t)=3+ceil(t/I)`、`V(t)=3+3 Lpeer(t+J_LLDP)`、`R(t)=2+Vpeer(t+J_Verify)`のcarry-inを含む。各controlには実xMII占有＋IPGに加え、admissionごとに8nsを予約する。全burstの `x >= D+B+H(x)` 固定点を求めるため、8nsをstreamごとに一度だけ加える計算にはしない。

planはsource/receiver ID、medium/attachment設定hash、sample grid、両端physical startup epoch、最初の2700symbol idle/PFC・seed alignment、directional観測Pmax、両方向Merge epoch・実capability identity/enabled/expiryと実Verify/Respond受信identityを保持する。prepare、installation、source admission、SOF、deliveryとphysical callbackの同一cutで実証明を照合する。readiness喪失はpacketの有無に依存せず両方向Mergeをresetし、pendingを取消し、適用済み保証を停止する。recoveryは新しいphysical/capability証明と明示prepareを要する。

gPTP等、追加control producerのarrival envelopeが未証明な要求は、該当requestだけ`ControlServiceEnvelope`で拒否する。自己記述したcheckpoint証明だけでは信頼せず、別processでfrozen inputの実committed prefixを再実行して照合する。一般のraw経路・PHY全profileの認証やIEEE全適合性は主張しない。N13と81件ledgerは継続中。
