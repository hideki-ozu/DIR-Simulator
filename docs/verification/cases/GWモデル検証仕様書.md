# GWモデル検証仕様書

文書バージョン：`0.1.0`
対象GitHubバージョン：`未リリース（main @ 7738b55）`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版。独立CANバス・静的GWの契約と検証を具体化 |

文書ID：`verification-gw`

| 項目 | 内容 |
| --- | --- |
| 状態 | 検証仕様確定。16組の静的入力と独立整数解析の照合を実施。製品の実行試験は未実行 |
| 実入力 | [scenarios.json](../fixtures/gw/scenarios.json)が各INIと期待projectionを列挙。INIは同名routing/workload JSONと[Types.ned](../fixtures/gw/models/gw/Types.ned)・Main/Multi/Chain networkを使用 |
| 静的照合 | `python3 docs/verification/fixtures/gw/verify_expectations.py`。JSON重複キー、参照ファイル、区間/所有者/循環、bit数・時刻・境界の解析値を照合 |
| 製品実行 | 実装後に`dir-simulator run --config docs/verification/fixtures/gw/delay.ini --output /tmp/dir-gw-delay`を実行。各INIごとに未存在の出力先を使用。results.json schema2のmodel_recordsからcan.request/can.receiver/gw.forwardのdataを取り出してprojectionへ照合する |
| 正本・判定 | [GW仕様](../../specs/models/GWモデル詳細機能仕様書.md)、[GW設計](../../design/GWモデル詳細設計書.md)。整数ps・bit・件数・ID・状態は完全一致。製品実装を期待値生成に使わず、CAN ID0空payload=50bit、ID1空payload=47bitと各遅延の加算から求める。実機・規格認証の証跡は対象外（注） |

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
| 実行結果 | 静的fixtureと解析期待値は照合済み。製品実行・内部journalレビューは未実施 |

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
| 期待 | queue: 元EOF100/210/320us、出力125kbps・容量1。copy0 SOF100/EOF500/release524us、copy1 SOF524/EOF924us、copy2は320usにqueue_full。容量0はattempt0でdrop。multicastはB=250kbpsでEOF300us、C=1MbpsでEOF150us。C容量0の場合もBはsuccess |
| 順序追試 | queueの3件を同一readyにする内部ready入力試験で、64件既定容量/65件目drop-newestと容量u32maxの境界を確認。同じ仲裁値は投入commit順、異なるIDはCAN優先度、送信中は待機容量から除く。キュー状態をjournalで照合 |
| 保存 | submitted行は生成成功を示す。満杯破棄はcan.request.data.status=dropped/drop_reason=queue_fullで判定し、元送信successを保持 |
| 実行結果 | 静的fixtureと解析期待値は照合済み。製品実行・内部journalレビューは未実施 |

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
| 実行結果 | 静的fixtureと解析期待値は照合済み。製品実行・内部journalレビューは未実施 |

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
| 実行結果 | 静的fixtureと解析期待値は照合済み。製品実行・内部journalレビューは未実施 |

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
| 期待 | forward_due=Tではprocessing forward1、child Request0。EOF=Tではcopy in_flight/eof=null、planned_eof=200us。queueではcreated3=submitted3+hop_drop0+processing0、Request success2/drop1。hopではcreated3=submitted2+hop_drop1+processing0 |
| 照合 | Request・Receiver・forwardはschema2 model_recordsのみへ格納。envelope last-transition時刻、参照ID、固定schema、sort、一意性、共通Recordの型/unit/null、元とcopyの別母数を照合。T±1psで処理の有無を確認 |
| 内部レビュー | 処理中GW・各Busキュー・FrameStore参照をsnapshot後に解放。原子commitの途中失敗を注入し、兄弟copyの半端な公開がなく直前prefixだけが出ることを確認 |
| 実行結果 | 静的fixtureと解析期待値は照合済み。製品実行・内部journalレビューは未実施 |

<a id="dir-test-0016"></a>

## 6. 既存入力と拡張契約の回帰

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
| 期待 | model-profile省略はidealのまま。既存schema1出力・CRC/stuff・仲裁・遅延・容量・終了projectionが同じ。Multibus aliasesは別固定descriptorで登録し、ideal payloadへ追加キーを出さない。multibusで1Busとgateways=[]も受理 |
| schema追試 | unknown profile/schema/kind/route field、JSON重複、欠落、floatやbooleanのID/hop、GW portへのnative generatorを診断。新profileの正規NED+INI+JSONを全体読込でき、旧キーは旧descriptorのまま保持 |
| コマンド | `python3 docs/verification/fixtures/can/verify_vectors.py`とGW checker、`python3 scripts/check_traceability.py --strict --requirement DIR-REQ-0123`。親子全体の追跡は0124～0132もstrict対象へ含める。実装後は両profileを同一buildで実行 |
| 実行結果 | 静的fixtureと解析期待値は照合済み。製品実行・内部journalレビューは未実施 |
