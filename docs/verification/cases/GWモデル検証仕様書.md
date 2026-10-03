# GWモデル検証仕様書

文書バージョン：`1.0.0`
対象GitHubバージョン：`v1.0.0`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.0.0` | `2026-10-03` | 有限RX・TX受理待ち、分岐、停止境界、Busゲート名自由化と比較証跡を追加。文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。独立CANバス・静的GWの契約と検証を具体化 |

文書ID：`verification-gw`

| 項目 | 内容 |
| --- | --- |
| 状態 | 検証仕様確定。[RX保持改訂後の実行記録](../results/gateway-rx-buffer-2026-10-03.json)で改訂16 fixtureと追加RX/TX境界・分岐・順序・終了状態を製品照合済み。従来の照合証跡と取得時のソースを区別する。全追試・外部拡張APIの適合は未完了 |
| 実入力 | [scenarios.json](../fixtures/gw/scenarios.json)が各INIと期待projectionを列挙。INIは同名routing/workload JSONと[Types.ned](../fixtures/gw/models/gw/Types.ned)・Main/Multi/Chain networkを使用 |
| 静的照合 | `python3 docs/verification/fixtures/gw/verify_expectations.py`。JSON重複キー、参照ファイル、区間/所有者/循環、bit数・時刻・境界の解析値を照合 |
| 製品実行 | `cargo test --locked -p dir-simulator --test gateway`が全16 fixtureを実行・拒否判定し、13結果のschema2公開も検証する。個別実行は`dir-simulator run --config docs/verification/fixtures/gw/delay.ini --output /tmp/dir-gw-delay`。各INIごとに未存在の出力先を使用 |
| 正本・判定 | [GW仕様](../../specs/models/GWモデル詳細機能仕様書.md)、[GW設計](../../design/GWモデル詳細設計書.md)。整数ps・bit・件数・ID・状態は完全一致。製品実装を期待値生成に使わず、CAN ID0空payload=50bit、ID1空payload=47bitと各遅延の加算から求める。実機・規格認証の証跡は対象外（注） |

RX保持導入前の2026-10-03の製品確認は、[gateway.rs](../../../crates/dir-simulator/tests/gateway.rs)の当時の6テストで実施した。fixtureの時刻・ID・状態に加え、schema2の参照・sort・初期状態・当時42指標descriptor、GW計測点と保存則、バス別受信・遅延・占有率、manifestハッシュ、入力順変更、Ready順、非発火の所有予約、桁あふれ・イベント上限での確定prefix、任意拡張子/BOMの入力出典を検証した。同じbuildで従来CANを含むRust 50テスト、ビューアモデル15テストとPlaywright操作試験が合格した。ビューアではBUSの同時送信、GW処理中・コピー選択・終端遅延、T境界、最新500転送行を確認した。これらの件数・合否は当時のソースに対する記録であり、本改訂のRX/TX待ち契約の合格証拠へ転用しない。

[導入前の実行結果記録](../results/gateway-2026-10-03.json)には基点commit、未コミット変更を含むソースhash、Rust/Node/Playwright/Linux環境、各コマンドの実行ログと合否、3バス分岐サンプルの実測を保存する。[先行する接続・受信ビューア記録](../results/viewer-rx-gateway-2026-10-03.json)も取得時のソースを示す。GWのOMNeT++比較はこれらの証跡に含まない。既存のv0.1単一CAN比較証跡は別の取得結果として保持する。

[RX保持改訂後の実行記録](../results/gateway-rx-buffer-2026-10-03.json)は変更後のソース・入力hash、コマンドと合否を保存する。`cargo test --offline --locked`の74試験（lib40、CAN5、CLI6、GW16、viewer CLI6、doc1）、Nodeモデル20試験（実GW結果13組を含む）、Playwright操作試験（ブラウザエラー・外部通信0）、Python53試験（比較アダプター14試験を含む）が合格した。fmt/clippy、GW静的16 fixture、strictトレーサビリティ193要件/48機能/225ノード（構造エラー・未完了0）、生成文書の`--check`も合格した。改訂queue、SOF時点の打切り、RX0/満杯・既定64/65件目/u32max、GW/TX処理中保持、分岐の先行・複数ingressのready順と入力並べ替え、イベント上限ごとの保持保存則、RX/TX受理時刻の巻き戻しを確認した。Pythonの比較アダプター試験はOMNeT++本体の実行を含まない。

[現行OMNeT++比較](../OMNeT比較結果.md#4-gatewayrx追加後の現行実装比較)では、上記の有効な通信条件を実際のOMNeT++ / FiCoにも投入した。GWの有効fixture13件、追加RX/TX・順序・gate名の入力変形15件、buffered-fanout例1件、native観測時刻による停止境界6件、計35ケースをDIRと対照した。GW処理は外部アダプター、CANは既存FiCoで実行し、origin/parent、route、RX保持／解放、TX待機、バス別占有・SOF順序と実payloadを検査した。件数ベクトル27件が一致したが、時間や停止状態を含む共通projectionの完全一致は送信なしの1件のみ。モデル差を補正せず、[生結果と判定](../results/omnet-current-2026-10-03.json)に保存した。

本比較後の確認ではRust74試験、viewerモデル21試験、Python69試験が合格した。Pythonには既存CAN比較器14試験と、実FiCoログの改変・入力対応に対するGW比較器16試験を含む。遅延RX解放、予定時刻内の転送欠落、SOF後の再投入欠落、FIFO追い越し、送信重複、payload/lineage破損を検出する。これらの回帰試験自体は取得済みログの検査であり、上記のOMNeT++本体156実行とは別に数える。

[buffered-fanout.ini](../../../examples/gateway/buffered-fanout.ini)の2ms実行はnative10件・コピー12件・Request計22件、RX released6件/dropped4件、RX最大2を確認した。450usのビューアではMain.gw.aのRX保持2/容量2とGW論理経路2本を確認できる。実行・表示コマンドは[README](../../../README.md#gatewayと複数can-busを実行する)を参照する。

以下の「追試」は仕様上の追加合格条件を残す。公開イベントcodecへの不正値注入、全内部状態のjournal APIレビュー、各遅延項の独立変化や全T±1psの組合せ、100万要求の性能はこの実行結果に含めない。準備失敗はCLIのE-0001診断であり、schema2の準備失敗成果物は未実装である。

<a id="dir-test-0011"></a>

## 1. 独立バスと処理・経路遅延

```trace
{
  "id": "DIR-TEST-0011",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0124",
    "DIR-REQ-0126"
  ],
  "upstream": [
    "design-gw#validation",
    "design-gw#routing-state",
    "design-gw#payload-export"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [validation](../../design/GWモデル詳細設計書.md#validation)、[routing-state](../../design/GWモデル詳細設計書.md#routing-state)、[payload-export](../../design/GWモデル詳細設計書.md#payload-export) |
| 上流 | [DIR-FUNC-0028](../../機能仕様書.md#dir-func-0028)、[DIR-AC-0028](../../要件定義書.md#dir-ac-0028)、[configuration](../../specs/models/GWモデル詳細機能仕様書.md#configuration) |
| 入力 | independent.ini、independent-only.ini、delay.ini。500kbps入力Busと250kbps出力Bus、ID0と出力側native ID1 |
| 期待 | independentはsource:0とother:0が同時SOF0。delayは元SOF3us/EOF103us、ingress observed108us/received115us、copy generated126us/ready139us。出力native ID1がrelease200usまで占有し、copy SOF200us/EOF400us。sink observed405us/received412us。path delay412us |
| 判定 | 各CAN Requestのbus/source/bitrate、元とコピーの50bit一致、別Busの重なったbusyを各々積分。独立-onlyはGW行0。delay項を一つずつ0へ変更し、該当項が一回だけ増減することを製品で確認 |
| 実行結果 | independent/independent-only/delayの製品時刻・同時SOF・転送bit数と、バス別占有率・受信数・平均遅延を照合済み。遅延各項を変更する追試は未実施 |

<a id="dir-test-0012"></a>

## 2. 分岐・出力容量・選出

```trace
{
  "id": "DIR-TEST-0012",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0125",
    "DIR-REQ-0127"
  ],
  "upstream": [
    "design-gw#routing-state",
    "design-gw#payload-export"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [routing-state](../../design/GWモデル詳細設計書.md#routing-state)、[payload-export](../../design/GWモデル詳細設計書.md#payload-export) |
| 上流 | [DIR-FUNC-0029](../../機能仕様書.md#dir-func-0029)、[DIR-AC-0029](../../要件定義書.md#dir-ac-0029)、[forwarding](../../specs/models/GWモデル詳細機能仕様書.md#forwarding) |
| 入力 | queue.ini、capacity-zero.ini、multicast.ini、multicast-drop.ini |
| 期待 | queue: 元EOF100/210/320us、出力125kbps・TX容量1。copy0 SOF100/EOF500/release524us、copy1はBUSY中の210usにTX受理、SOF524/EOF924/release948us。copy2はready320usでwaiting_txとなりRXを320～524us保持、copy1のSOFによる空き通知でtx_enqueued524us、SOF948/予定EOF1348us。T=1msならcopy2はin_flightで実EOF=null。TX容量0はattempt0でqueue_full。multicastはB=250kbpsでEOF300us、C=1MbpsでEOF150us。C容量0の場合もBはsuccess |
| 順序・容量 | 同時readyと複数ingressから同一egressへ到着する入力で、初回ready event順による待ち受理と既存CAN優先度を別に確認する。BUSY中の空き受理、SOFの除去直後の次delta phase1起床、再試行後もcopy ID・Request・timerが一つで遅延を再加算しないことを確認。RX既定64で保持64件と65件目rx_queue_full、RX/TX容量1・u32maxを確認。TX送信中・processing・waiting_txはTX待機容量から除く |
| RX境界 | rx_queue_capacity=0は全新着をrx_queue_full、1で保持中の次着は新着のみ拒否。元CAN success/Receiver.receivedを保持し、拒否行のreleased_ps=null、転送行/childなし、意図したegressを保存。routes=[]の受理はno_routeで同時刻RX解放。RX0/満杯では経路不一致でもrx_queue_fullのみでno_route転送行を作らない。RX負・u32max+1・float・boolean・string・nullは準備拒否、キー省略64と0/u32max整数は受理 |
| 分岐 | BのTXを満杯にしCを空きにした入力ではCの受理・送信を先行させ、親RX一枠をB受理まで保持。RX容量1で後続を投入するとrx_queue_fullは親一件として両枝の転送を抑止する。TX0枝と正常枝は個別に終端/受理し、最後の枝の完了時だけRXを一回解放。GW処理・各TX処理を非ゼロにして独立した処理中の保持も確認 |
| 保存 | submitted行は生成成功を示し、waiting_tx中もsubmittedのまま。TX0破棄はcan.request.data.status=dropped/drop_reason=queue_full、RX満杯はgw.rx_buffer.status=dropped/reason=rx_queue_fullで判定する。RX破棄はCAN droppedに加えず元送信successを保持。tx_enqueued−readyの待ちは実受理時に一回だけコピーIDへ記録 |
| 実行結果 | [改訂後記録](../results/gateway-rx-buffer-2026-10-03.json)でqueueのRX保持・copy2 in_flight、RX0/満杯/64・65件/u32max、TX0、no_route、独立GW/TX処理、分岐の先行と共有RX飽和、複数ingressのready順・入力並べ替え、容量型/範囲の拒否を確認。再試行後もコピー一意性を保持する。内部journal APIの直接注入による追試は未実施 |

<a id="dir-test-0013"></a>

## 3. 範囲・所有者・経路循環

```trace
{
  "id": "DIR-TEST-0013",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0125",
    "DIR-REQ-0129",
    "DIR-REQ-0130"
  ],
  "upstream": [
    "design-gw#validation"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [validation](../../design/GWモデル詳細設計書.md#validation) |
| 上流 | [DIR-FUNC-0028](../../機能仕様書.md#dir-func-0028)、[DIR-AC-0028](../../要件定義書.md#dir-ac-0028)、[configuration](../../specs/models/GWモデル詳細機能仕様書.md#configuration) |
| 入力 | invalid-overlap.ini、invalid-owner.ini、invalid-cycle.ini、disjoint-cycle.ini |
| 期待 | 重複端点10、別sourceのID0所有、同format/idのA→B→Aはそれぞれ準備失敗。standard A→Bとextended B→Aは受理し要求0。prepare不正はrunイベント0、原因route/fieldを診断 |
| 範囲追試 | route min=max=0/2047 standard、0/536870911 extendedは受理。min>max、負、上限+1、unknown format、重複egress、ingressへの自己出力、未知port、他GW所属port、同Bus所属2portは準備失敗。count=0又は区間外generatorでも所有者交差を拒否 |
| 解析独立性 | 仕様設計の端点分割に対してfixture checkerは経路の区間を逐次交差させるDFSで閉路を確認。ID全域を列挙する必要はない |
| 実行結果 | 3拒否fixtureと異format循環受理を製品照合。非発火native予約、数値・型・重複キー・format・自己egress・未知portも拒否確認。全位置情報の構造化診断は未実装 |

<a id="dir-test-0014"></a>

## 4. 識別・hop・非適合受信

```trace
{
  "id": "DIR-TEST-0014",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0125",
    "DIR-REQ-0128"
  ],
  "upstream": [
    "design-gw#routing-state",
    "design-gw#payload-export"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [routing-state](../../design/GWモデル詳細設計書.md#routing-state)、[payload-export](../../design/GWモデル詳細設計書.md#payload-export) |
| 上流 | [DIR-FUNC-0029](../../機能仕様書.md#dir-func-0029)、[DIR-AC-0029](../../要件定義書.md#dir-ac-0029)、[forwarding](../../specs/models/GWモデル詳細機能仕様書.md#forwarding) |
| 入力 | hop.ini、no-route.ini、rx-filter.ini、multicast.ini |
| 期待 | 3段・上限2では100usにhop1、200usにhop2のcopy生成、300usにhop3転送行をdropped_hop_limitで終端。第3copy Requestは存在せず、元と前2copyはsuccess。no-routeはReceiver.receivedとGW.filtered/no_route、rx-filterはReceiver.filteredのみでGW行0 |
| 識別 | origin=source:0を全copyで維持。parentは直前Request、copy IDはegressごと一意。payload、CRC、frame_bitsは全copy一致。標準default16とhop_limit 1/65535受理、0/65536拒否をprepare試験 |
| 不変条件追試 | 重複RoutingInput、parent/origin/frame不一致、unknown forward_id、誤時刻はmodel_failed、壊れたcodec fieldはinvalid_event。最後のcommit prefixを保持 |
| 実行結果 | hop/no-route/rx-filter/multicastの製品状態・時刻・参照とフレーム保持を照合。hop上限の値域、出力のreason/receiverとビューアの不正参照拒否を確認。外部codec・内部不変条件への直接注入は未実施 |

<a id="dir-test-0015"></a>

## 5. 終了境界・台帳・記録

```trace
{
  "id": "DIR-TEST-0015",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0126",
    "DIR-REQ-0127",
    "DIR-REQ-0128",
    "DIR-REQ-0131"
  ],
  "upstream": [
    "design-gw#routing-state",
    "design-gw#payload-export"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [routing-state](../../design/GWモデル詳細設計書.md#routing-state)、[payload-export](../../design/GWモデル詳細設計書.md#payload-export) |
| 上流 | [DIR-FUNC-0030](../../機能仕様書.md#dir-func-0030)、[DIR-AC-0030](../../要件定義書.md#dir-ac-0030)、[payload-results](../../specs/models/GWモデル詳細機能仕様書.md#payload-results) |
| 入力 | forward-boundary.ini（T=120us、GW delay20us）、eof-boundary.ini（T=200us）、queue.ini、hop.ini |
| 期待 | forward_due=Tではprocessing forward1、child Request0、RX holding1。EOF=Tではcopy in_flight/eof=null、planned_eof=200us。queueではcreated3=submitted3+hop_drop0+processing0、コピーRequest success2/in_flight1、RX released3/holding0/drop0。hopではcreated3=submitted2+hop_drop1+processing0でhop破棄時にRX解放 |
| 照合 | Request・Receiver・forward・rx_bufferはschema2 model_recordsのみへ格納。envelope last-transition時刻、参照ID、固定schema、sort、一意性、共通Recordの型/unit/null、元とcopyとRXの別母数を照合。TX受理・SOF・RX解放のT±1psで処理の有無、waiting_tx summaryとgenerated/unfinished保存則を確認。TX処理中とwaiting_txを別状態として終了snapshotに残す |
| ビューア | 無負荷・未送信Controllerを含むtopologyのController–Bus接続とController間GW論理経路を確認。320usのqueueのRX件数/容量とheld parent、524usのRX解放、TX受理前後のwaiting_tx/pending、1msのin_flightを確認。時刻を戻すと同じRX枠/要求が復元され、残像・最新500行の制限で件数を変えないことをモデル試験とブラウザ操作で確認 |
| 内部レビュー | 処理中GW・各Busキュー・FrameStore参照をsnapshot後に解放。原子commitの途中失敗を注入し、兄弟copyの半端な公開がなく直前prefixだけが出ることを確認 |
| 実行結果 | [改訂後記録](../results/gateway-rx-buffer-2026-10-03.json)でforward-boundary/eof-boundaryとSOF時点打切り、TX処理中/waiting_txを含むイベント上限ごとのRX枝保存則、model_records/CSV/manifestと計測保存則を確認。ビューアでは524usの実TX受理境界、保持親と兄弟枝の先行、正確な接続、idle Controller、再生/移動/巻き戻しを確認。全T±1ps組合せ・内部journal API直接注入は未実施 |

<a id="dir-test-0016"></a>

## 6. 既存入力と拡張契約の回帰

v1.0.0のBusゲート名自由化について、旧名・任意名での実行結果と遅延の一致、別Busへの片側接続の拒否を追加確認した。[検証記録](../results/can-gate-names-2026-10-03.json)はこの変更後のソース・入力hashと実行結果を記録し、先行するGW検証証跡とは区別する。

```trace
{
  "id": "DIR-TEST-0016",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0123",
    "DIR-REQ-0124",
    "DIR-REQ-0125",
    "DIR-REQ-0129",
    "DIR-REQ-0130",
    "DIR-REQ-0132"
  ],
  "upstream": [
    "design-gw#validation",
    "design-gw#payload-export"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 担当設計 | [validation](../../design/GWモデル詳細設計書.md#validation)、[payload-export](../../design/GWモデル詳細設計書.md#payload-export) |
| 上流 | [DIR-FUNC-0030](../../機能仕様書.md#dir-func-0030)、[DIR-AC-0030](../../要件定義書.md#dir-ac-0030)、[payload-results](../../specs/models/GWモデル詳細機能仕様書.md#payload-results) |
| 入力 | 既存[CAN fixture](../fixtures/can/scenarios.json)全8ケース、[CAN vectors](../fixtures/can/vectors.json)、全GW fixture |
| 期待 | model-profile省略はidealのまま。既存schema1のRequest/CanFields・metric集合とCRC/stuff・仲裁・遅延・容量・終了projectionが同じ。topology.controllersだけは両schemaに正確な所属とTX/RX経路遅延を追加。Multibus aliasesは別固定descriptorで登録し、ideal payloadへTX受理時刻やwaiting_txキーを出さない。multibusで1Busとgateways=[]も受理 |
| schema追試 | unknown profile/schema/kind/route field、JSON重複、欠落、floatやbooleanのID/hop、GW portへのnative generatorを診断。新profileの正規NED+INI+JSONを全体読込でき、旧キーは旧descriptorのまま保持 |
| v1.0.0 Bus命名自由化 | delay fixtureのBusゲートを任意名・旧名へそれぞれ変更し、各ControllerのBus所属・非ゼロ経路遅延と実行イベント・転送記録・計測値が元入力と一致することを確認。入出力の役割は名前や宣言順によらず方向で決定する。元入力snapshot/hashと名前を含む接続識別は改名に従う。Bus間で受信経路を交換した片側別Bus接続と、Bus間の直接接続は準備失敗とする |
| コマンド | `python3 docs/verification/fixtures/can/verify_vectors.py`とGW checker、`python3 scripts/check_traceability.py --strict --requirement DIR-REQ-0123`。親子全体の追跡は0124～0132もstrict対象へ含める。実装後は両profileを同一buildで実行 |
| 実行結果 | [改訂後記録](../results/gateway-rx-buffer-2026-10-03.json)で既定CANの既存Rust試験と改訂全GW fixture、schema1/2ビューア操作が同一buildで合格。比較アダプター14試験は旧名/新名/任意Busゲート名・経路遅延の正規化と不正方向/接続の拒否を確認。続く[現行比較記録](../results/omnet-current-2026-10-03.json)ではCAN43/GW35ケースを両エンジンで実行した。公開Registry/Envelopeのdescriptor・codec API適合は未実施 |
