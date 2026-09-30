# Ethernetモデル検証仕様書

文書バージョン：`0.1.0`
対象GitHubバージョン：`未リリース（main @ 7738b55）`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.0` | `2026-10-01` | 作業内容を集約：4frame vector・12入力集合・解析期待値と製品検証手順を追加。親要件の交換責務を追跡し、共通profile・codec登録境界と照合。CSMA/CD半二重・1000BASE-T1の媒体別profileと互換境界を追加 |

文書ID：`verification-ethernet`

| 項目 | 内容 |
| --- | --- |
| 適用 | 本書の基準契約はethernet.l2.store-forward.v1。半二重と1000BASE-T1はv2の[媒体検証](Ethernet媒体拡張検証仕様書.md)を併用 |
| 状態 | 仕様・設計・検証入力を具体化。製品実装・製品試験は未実施 |

| 項目 | 内容 |
| --- | --- |
| fixture | [Main.ned](../fixtures/ethernet/models/ethdemo/Main.ned)、[model.json](../fixtures/ethernet/model.json)、[scenarios.json](../fixtures/ethernet/scenarios.json)、[vectors.json](../fixtures/ethernet/vectors.json)。全12組のINI/workloadを格納 |
| static検証 | `python3 docs/verification/fixtures/ethernet/verify_fixtures.py`。CRC/frame4件、JSON/INI参照12組、閉形式の経路時間、copy保存則を検査する。NEDは入力文書のレビュー対象で、製品NED parserを実行した証拠とは区別 |
| 製品実行 | 実装後に`dir-simulator run --config docs/verification/fixtures/ethernet/unicast.ini --output /tmp/dir-ethernet-unicast`。caseごと別の未作成outputを使い、`python3 docs/verification/fixtures/ethernet/verify_fixtures.py --scenario unicast --results /tmp/dir-ethernet-unicast/results.json`で指定projectionを照合 |
| 判定 | CRC/bytes/件数/ps/状態は完全一致、共通浮動指標は結果仕様のbinary64値で照合。projectionにないevent sequence絶対値の推測は期待値生成の対象外（注）。診断caseはCLI exitとstderrも別途確認 |
| 証拠境界 | 下記は解析fixtureと試験仕様。製品実行・故障注入・IEEE規格認証を合格扱いにする記録は未取得 |

<a id="dir-test-0023"></a>

## 1. CRC・padding・MAC長

```trace
{
  "id": "DIR-TEST-0023",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0146",
    "DIR-REQ-0147",
    "DIR-REQ-0149"
  ],
  "upstream": [
    "design-ethernet#serialization"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 上流 | [DIR-AC-0034](../../要件定義書.md#dir-ac-0034)、[Ethernet仕様](../../specs/models/Ethernetモデル詳細機能仕様書.md)、[詳細設計](../../design/Ethernetモデル詳細設計書.md) |
| 入力 | vectors.jsonのempty/one-byte/minimum-payload/maximum-payload、source02:00:00:00:00:01、destination02:00:00:00:00:02、EtherType0800 |
| 期待 | L0/1/46はMAC64byte、pad46/45/0。L1500はMAC1518byte、pad0。CRC32 check vector123456789はcbf43926、FCS wire順は2639f4cb。全frame固有FCS/mac_hexはvectors.jsonの固定値と一致 |
| 時間 | 1Gbps、MAC64→wire576bit/EOF576000ps、IFG96bitを含めrelease672000ps。MAC1518→wire12208bit/EOF12208000ps、release12304000ps。100Mbps/10Mbps/10Gbpsにも同じ整数式を適用 |
| 独立性 | fixture生成にはPython zlibを用い、verify側は各入力bitを読む別CRCレジスタ式を用いる。製品serializerから期待値を採取する方式は対象外（注）。payload/FCS/paddingとpreambleを一度ずつ数えることをレビュー |
| 製品判定 | encode_frame APIと結果ethernet.frame.dataを固定bytesで比較。L1501、odd hex、source group、VLAN EtherTypeはprepare失敗を確認 |

<a id="dir-test-0024"></a>

## 2. 全二重・伝搬・蓄積交換時間

```trace
{
  "id": "DIR-TEST-0024",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0148",
    "DIR-REQ-0149"
  ],
  "upstream": [
    "design-ethernet#prepare",
    "design-ethernet#serialization",
    "design-ethernet#forwarding"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 上流 | [DIR-AC-0034](../../要件定義書.md#dir-ac-0034)、[Ethernet仕様](../../specs/models/Ethernetモデル詳細機能仕様書.md)、[詳細設計](../../design/Ethernetモデル詳細設計書.md) |
| 入力 | unicast.iniとduplex.ini、1Gbps、各方向delay1000ps、switch forward2000ps、endpoint処理0 |
| 期待 | sourceSOF0/EOF576000/release672000/arrival577000、switch出力SOF579000/EOF1155000/release1251000、receiver1156000ps。逆方向要求も同時に同じ時刻で成立 |
| 判定 | unicastのtransfer_timesとdeliveries、duplexの反対方向占有の重なりと独立二配送をprojection checkerで比較。互いのqueue/ownerを占有する処理がないことを内部レビュー |
| 追加境界 | 正delayのendpoint tx=3000ps/rx=4000psへ一箇所ずつ変更し、その分だけ最終受信が移動すること。伝搬をwire busyへ加算しないこと、switchが受信FCS到達前にSOFを作らないことを確認 |

<a id="dir-test-0025"></a>

## 3. 静的FDB・broadcast・未知unicast

```trace
{
  "id": "DIR-TEST-0025",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0151",
    "DIR-REQ-0152",
    "DIR-REQ-0153"
  ],
  "upstream": [
    "design-ethernet#forwarding"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 上流 | [DIR-AC-0035](../../要件定義書.md#dir-ac-0035)、[Ethernet仕様](../../specs/models/Ethernetモデル詳細機能仕様書.md)、[詳細設計](../../design/Ethernetモデル詳細設計書.md) |
| 入力 | unicast.ini、broadcast.ini、unknown-unicast.ini、Main.swの静的FDB |
| 期待 | unicastはsourcecopy1+出力copy1、broadcastはsourcecopy1+出力copy2と受信2、unknownは同じ3copyとdestination_mismatch2。input側へ戻るcopy0。全child parent_transfer_idはsourcecopyを指す |
| 同入力filter | model.jsonの宛先bのegressをMain.sw.tx_aへ置換し実行。sourcecopy1、switch reception filtered/same_ingress、出力copy0、受信0。全入力内容を保存して元fixtureとの相違を一つに限定 |
| 判定 | 全候補のport順、immutable bytes共有、static FDBが実行中不変、unknownを黙ってdropへ置換しないことを確認。treeとingress除外による有限複製もレビュー |

<a id="dir-test-0026"></a>

## 4. 有限FIFOと同時刻容量判定

```trace
{
  "id": "DIR-TEST-0026",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0148",
    "DIR-REQ-0150"
  ],
  "upstream": [
    "design-ethernet#forwarding"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 上流 | [DIR-AC-0035](../../要件定義書.md#dir-ac-0035)、[Ethernet仕様](../../specs/models/Ethernetモデル詳細機能仕様書.md)、[詳細設計](../../design/Ethernetモデル詳細設計書.md) |
| 入力 | source-full.ini、egress-full.ini、zero-capacity.ini |
| 期待 | source-fullは同時2生成でa:1@Main.a.txだけqueue_full、a:0配送。egress-fullはa/cが同時にswitch到達し、Main.sw.tx_b容量1へa:0が先、c:0@Main.sw.tx_bだけdrop。zeroはsourcecopy1/drop1、送信0 |
| phase境界 | source-fullで時刻0のphase1に二offer、phase2で取出し。同時刻の後段取出しを先取りして2件とも受理する処理は対象外（注） |
| 追加FIFO | sourceのtimes_ps=[0,1000,2000]、容量2へ変更。最初SOF0、続いて672000、1344000ps。送信中の一件がqueue容量へ加わらないこと。copy保存則とqueue最大2を照合 |
| fanout部分満杯 | broadcast受信時にtx_bだけ既存待機で満杯の内部状態を準備し、tx_bはdrop、tx_cはacceptedの同commitを確認。通常queue_fullと実行失敗のrollbackを区別 |

<a id="dir-test-0027"></a>

## 5. 準備拒否と決定的負荷

```trace
{
  "id": "DIR-TEST-0027",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0146",
    "DIR-REQ-0154"
  ],
  "upstream": [
    "design-ethernet#prepare"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 上流 | [DIR-AC-0036](../../要件定義書.md#dir-ac-0036)、[Ethernet仕様](../../specs/models/Ethernetモデル詳細機能仕様書.md)、[詳細設計](../../design/Ethernetモデル詳細設計書.md) |
| 入力 | bad-fdb.ini、bad-payload.iniと各正常fixture |
| 期待 | bad-fdbはE-0001/invalid_connection、bad-payloadはE-0001/invalid_range、prep_failed/exit2/callback0。完全fixtureを一箇所だけ変えた原因と対象pathを記録 |
| 拒否拡張 | 既存完全入力から、JSON duplicate key、未知field、times降順、負/先頭0D、EtherType bool、MAC重複、FDB duplicate、unknown kind、逆方向未接続、reverse rate不一致、cycle、2peerへのEndpoint接続を一つずつ注入。全て準備境界で停止 |
| 正常境界 | times空、同時刻重複、Tと等しい時刻、T以後だけの負荷、未知unicastは受理。T以後生成0。generator配列反転とJSON field順変更後にもframe ID/時刻/複製順が同じ |
| 参照 | profile版・model-config raw入力・負荷raw入力・初期stateがmetadataへ保存されること。CAN既定入力を同じregistryへ渡して元profileの結果が変わらないこと |

<a id="dir-test-0028"></a>

## 6. 停止prefix・fanout原子性・結果

```trace
{
  "id": "DIR-TEST-0028",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0145",
    "DIR-REQ-0155",
    "DIR-REQ-0156"
  ],
  "upstream": [
    "design-ethernet#observations",
    "design-ethernet#forwarding"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 内容 |
| --- | --- |
| 上流 | [DIR-AC-0036](../../要件定義書.md#dir-ac-0036)、[Ethernet仕様](../../specs/models/Ethernetモデル詳細機能仕様書.md)、[詳細設計](../../design/Ethernetモデル詳細設計書.md) |
| 入力 | eof-boundary.ini T576000、arrival-boundary.ini T577000、switch-processing.ini T577001 |
| 期待 | EOF境界はtransmitting1/serialized0/reception0、arrival境界はserialized1/reception0、switch-processingはserialized1/reception processing1/ready null。全てtime_limit正常、予定時刻と実績を区別 |
| 追加時間 | receiver ready時刻1156000psで打切りならreceiver到達/完了イベントを未処理、1156001psでreceived。T0ならframe/transfer/reception行0、queue最大0、率null |
| 失敗注入 | Switch fanoutの最後の予約又はjournal検証をテストContextで失敗させる。execution_failed/partial true、子copyとqueue増分は全0、親は直前commitのprocessingのまま、失敗候補はpending。既に成功した別frameの結果を保持 |
| 内部/出力判定 | offersの4状態保存則、arrival=reception数、親子ID一意、各実績時刻が予定と一致、finish後snapshot不変。共通manifest hashとCSV/JSONの整合を既存結果試験へ接続 |
| 実行結果 | 本書作成時はstatic verifierのみ実行。製品の終了・失敗注入・資源解放は未実行 |

## 7. 実行記録

| 日付・対象 | 状態と証拠 |
| --- | --- |
| 2026-09-29・解析fixture | Python 3標準ライブラリのverify_fixtures.pyを実行。4 CRC/frame vector・12入力集合・経路の閉形式時間がPASS。製品ソースを期待値算出に用いる処理は対象外（注） |
| 未実行 | DIR-Simulator製品による12scenario実行、追加mutation、失敗注入、性能、IEEE適合認証。実装後は対象commit・実行環境・入力hash・期待値・実測値・判定・成果パスを本表へ追記 |
