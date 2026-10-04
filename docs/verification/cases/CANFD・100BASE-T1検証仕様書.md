# CANFD・100BASE-T1検証仕様書

文書バージョン：`1.1.2`
対象GitHubバージョン：`main @ 7bb9bfc`
予定公開版：`v1.1.2`（本PR。対象コミットは公開済みmainの基準）

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.2` | `2026-10-04` | FD/T1の製品結果checker、時間・仲裁・停止・入力・viewer試験と実行記録を追加 |
| `1.0.0` | `2026-10-03` | 文書版を1.0.0、対象タグをv1.0.0に統一 |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版草案。CAN FD・100BASE-T1の実装契約と独立解析fixture |

文書ID：`verification-original-network`

状態：採用した抽象profileの契約を確定。開発中ソースに製品実装を追加。製品試験の実施範囲は本書の検証記録又は対応する検証仕様を参照。規格全体適合・実機試験は未実施。文書版を1.1.2に統一し、公開版はv1.1.2向けPRとして準備する。独立解析fixtureと製品実行の証跡は区別する。

| 項目 | 入力と検証境界 |
| --- | --- |
| 解析 | [vectors.json](../fixtures/original-network/vectors.json)、[verify_expectations.py](../fixtures/original-network/verify_expectations.py)。`python3 docs/verification/fixtures/original-network/verify_expectations.py`で42解析ケースと2構成入力を確認する |
| FD構成 | [fd.ini](../fixtures/original-network/fd.ini)、[Main.ned](../fixtures/original-network/models/fd/Main.ned)、[fd.model.json](../fixtures/original-network/fd.model.json)、[fd.workload.json](../fixtures/original-network/fd.workload.json)。64byte synthetic位相長N40/D600、公称500kbps/データ2Mbps。EOF380000000ps、release386000000ps |
| T1構成 | [t1.ini](../fixtures/original-network/t1.ini)、[t1.model.json](../fixtures/original-network/t1.model.json)、[t1.workload.json](../fixtures/original-network/t1.workload.json)。既存ethernet-media/modelsのMain/Types.nedを参照する |
| 静的検査 | JSON重複キー・対象構造・profile・速度・wire binding・計算結果とINIのfile参照を確認。NEDはファイル存在を確認するだけで、parserやruntime試験ではない |
| 製品試験 | `dir-simulator run --config docs/verification/fixtures/original-network/fd.ini --output /tmp/dir-fd`、T1はt1.iniと別出力へ置換する。[verify_results.py](../fixtures/original-network/verify_results.py)へ`--result fd=OUTPUT_ROOT --result t1=OUTPUT_ROOT`を渡し、schema2結果を照合する。全時刻・ID・状態は完全一致。内部試験と診断の実施範囲は[実行記録](#product-execution)を参照 |

<a id="dir-test-0040"></a>

## 1. FD時間と入力

```trace
{
  "id": "DIR-TEST-0040",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0174",
    "DIR-REQ-0175",
    "DIR-REQ-0176",
    "DIR-REQ-0177",
    "DIR-REQ-0178",
    "DIR-REQ-0179",
    "DIR-REQ-0180",
    "DIR-REQ-0181"
  ],
  "upstream": [
    "design-original-network#canfd"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 検証契約 |
| --- | --- |
| 入力・期待・製品検証境界 | BRS true/false、64byte、二速度比とceil境界、DLC全対応長を検証する。長さ9/63/65、ID範囲外、data_bits不足、BRS=falseでD>0、証跡空、速度逆転を拒否する。製品側は未知/重複/欠落キー、型違反、u64時刻overflowのコードと非commitを確認する。 |
| 受入 | 担当受入：[DIR-AC-0043](../../要件定義書.md#dir-ac-0043)。解析fixtureの検証結果と製品試験結果を別に記録する。 |

<a id="dir-test-0041"></a>

## 2. FD仲裁・queue・停止

```trace
{
  "id": "DIR-TEST-0041",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0174",
    "DIR-REQ-0175",
    "DIR-REQ-0176",
    "DIR-REQ-0177",
    "DIR-REQ-0178",
    "DIR-REQ-0179",
    "DIR-REQ-0180",
    "DIR-REQ-0181"
  ],
  "upstream": [
    "design-original-network#canfd"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 検証契約 |
| --- | --- |
| 入力・期待・製品検証境界 | 同時ID0/1を仲裁してSOF0/106000000ps、EOF100000000/206000000ps。停止T=EOFではtransmitting、EOF+1ではserialized。製品側はstandard/extended混在、後着高優先度、同一ID所有者競合、容量drop-newest、tx/rx遅延、同報、受信=T、release=T、失敗prefixを確認する。synthetic位相長が合法なCAN FD波形であるとの検証は行っていない。 |
| 受入 | 担当受入：[DIR-AC-0043](../../要件定義書.md#dir-ac-0043)。解析fixtureの検証結果と製品試験結果を別に記録する。 |

<a id="dir-test-0042"></a>

## 3. 100T1双方向とPHY

```trace
{
  "id": "DIR-TEST-0042",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0182",
    "DIR-REQ-0183",
    "DIR-REQ-0184",
    "DIR-REQ-0185"
  ],
  "upstream": [
    "design-original-network#t1"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 検証契約 |
| --- | --- |
| 入力・期待・製品検証境界 | 64byte MACをSOF0から双方向同時送信しEOF5760000ps、release6720000ps。P1000、A TX100000/RX400000、B TX300000/RX200000ならA→B arrival6061000、B→A arrival6461000ps。1518byte境界、方向交換、PHY0を解析する。製品側はFDB転送と後続FIFO、二方向独立、処理遅延の一回加算を確認する。 |
| 受入 | 担当受入：[DIR-AC-0044](../../要件定義書.md#dir-ac-0044)。解析fixtureの検証結果と製品試験結果を別に記録する。 |

<a id="dir-test-0043"></a>

## 4. 100T1検証・停止・互換

```trace
{
  "id": "DIR-TEST-0043",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0182",
    "DIR-REQ-0183",
    "DIR-REQ-0184",
    "DIR-REQ-0185"
  ],
  "upstream": [
    "design-original-network#t1"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 検証契約 |
| --- | --- |
| 入力・期待・製品検証境界 | 100Mbps/full/master-slaveを受理し1Gbps、half、同roleを拒否する。T=EOFは未成功、T=arrivalは未受信、arrival+1で受信を解析する。製品側はrole/遅延欠落・重複link/port・未知キーとoverflowを拒否し、旧v1/v2 fixture結果の無変更、失敗prefix、metadataの正しいprofileを確認する。 |
| 受入 | 担当受入：[DIR-AC-0044](../../要件定義書.md#dir-ac-0044)。解析fixtureの検証結果と製品試験結果を別に記録する。 |

<a id="dir-test-0097"></a>

## 5. FD証跡・bindingの変更検出と結果保存

```trace
{
  "id": "DIR-TEST-0097",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0179",
    "DIR-REQ-0180"
  ],
  "upstream": [
    "design-original-network#canfd"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 入力・期待・検証境界 |
| --- | --- |
| 再現入力 | fd.ini/model/workloadとmodels/fd/Main.nedを一時ディレクトリへ複製して参照をそろえる。基準はstandard/id0、data=a5の64byte、brs=true、N40/D600、Rn500000/Rd2000000、binding=c7ef80bcb1e130f62f3278f381fbfc72848ec6661bba94c4b7e4b5bc6f703b0e。evidenceはfixtureの文字列を保持 |
| 結合式 | ASCII `standard\|0\|` + `a5`64回 + `\|1\|40\|600\|500000\|2000000`（各区切りはbackslashを含まないpipe一文字）のSHA-256。入力dataの大小はlowercaseへ正規化し、整数は先頭0なし十進で結合 |
| 改変5件 | 各試験は基準から一項目だけ変更し、元のbindingを保持する。(1)data先頭a5→a4（長さ64byte維持）、(2)N40→41、(3)D600→601、(4)Bus nominalBitrate500kbps→1Mbps、(5)dataBitrate2Mbps→4Mbps。全件が独立には型/範囲/phase下限を満たすがbinding不一致でprepare E-0001、details.ruleと対象を記録、initialize/callback0、結果model_records空 |
| 正常対照 | 上記5件ごとに変更後の8fieldからbindingを独立に再算定するとprepare受理。duration/releaseは順に380000000/386000000、382000000/388000000、380500000/386500000、340000000/343000000、230000000/236000000ps。元hash不一致の検出と一般的な構造拒否を混同しない |
| 再結合hash（改変1） | 2f2cce63ee77077feab871160ba36f33e35eacb5f8f3e3b3e74147c13bc3a7f3 |
| 再結合hash（改変2） | 4b30a2eefadf91e58a36c70756755f9e1c98657c397123a6ee6b8a36f49559d2 |
| 再結合hash（改変3） | 9143d30862d0bd98c0568fe96684bd7fbfcd7220336e6025f59c541e9f0449c2 |
| 再結合hash（改変4） | bde8011294957604245082fc8b20ee8d2394d9a445eec448bb58f88f788bfb47 |
| 再結合hash（改変5） | f1b88482bbbd0374cbacf1f249f8281f7f25c7b754af5e01f384da5050b906bf |
| 静的行 | 全prepare成功後のdir.canfd.frameにnormalized data、導出dlc15、N/Dの十進文字列、evidence完全一致、変更後hash、nominal_rate/data_rate完全一致を確認する。fidelity=externally-precomputed-phase-bits、wire_validation=structural-only。hashはframe内容/速度との整合を確認する値でありbitstreamのISO適合証拠ではない |
| 証跡境界 | evidence空・513文字はprepare拒否。合法なevidence文字列だけを変更する場合、hashにevidenceは含まれないのでそのまま受理して変更文字列を保存する。内容の信頼性は利用者の独立検証対象。synthetic文字列を測定済み又は合法波形へ改称しない |
| 内部判定 | normalize→8field結合→hash比較→checked時間式→全prepare成功後のframe cache/journal公開を確認する。失敗途中の静的行を公開しない。DIR-AC-0043。独立hash/算術照合と製品prepare・結果試験を実施。改変表の全組合せと公開journal APIの追試は残る。範囲は[実行記録](#product-execution)を参照 |

<a id="dir-test-0098"></a>

## 6. FD一致/不一致フィルタと受信停止

```trace
{
  "id": "DIR-TEST-0098",
  "stage": "verification",
  "requirements": [
    "DIR-REQ-0178",
    "DIR-REQ-0180"
  ],
  "upstream": [
    "design-original-network#canfd"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 入力・期待・検証境界 |
| --- | --- |
| 完全入力差分 | fd.iniの三Controller構成と64byte frameを継承。aの生成0、txProcessingDelay0、全channel delay0、b.rxFilter="std:0x0"、c.rxFilter="std:0x1"、b/c.rxProcessingDelay=10ps、他既定値。負荷はaだけ。EOF380000000、release386000000、b/c planned_arrival380000000、planned_completed380000010ps |
| EOF等号 | T380000000ではtransmitting、eof/release実績null、受信行0、serialized0/received0。予定EOF/releaseは保持する |
| 到達と拒否 | T380000001では要求serialized、EOF実績380000000、release実績null。bはarrival380000000・processing・completed=null、cはarrival380000000・filtered・completed=null。両者のplanned_arrival/planned_completedは上記のまま。filteredはdrop_reasonや要求dropへ移さない |
| 完了等号 | T380000010ではbのcompletionは未処理でprocessing、cはfiltered、receivedは全target0。T380000011ではbのみcompleted、completed_ps380000010、canfd.receivedのb/$all=1、a/c=0。serializedはa/$all1、generatedはa/$all1、dropped全0 |
| 全拒否対照 | b/c.rxFilter="none"へ変更しT380000011。両受信行filtered、arrival380000000、completed=null、planned_completed380000010を保持。received0でも理想ACKとEOF serialized1は成立 |
| 全一致対照 | b/c.rxFilter="*"、rxProcessingDelay0へ変更しT380000001。Arrival batchで両者completed、arrival=completed380000000、receivedのb/c各1、$all2、送信元a0、serializedは1。自己配送を追加しない |
| 内部判定 | EOFのreception作成、Arrivalの(format,id)判定、拒否時にcompletion timerが0件、適合時一件、同じEOF/request世代の参照を採取。予定値を実績へ補完しない。FDの4 descriptorだけを保持し、CCのfiltered指標を暗黙追加しない |
| 受入・状態 | DIR-AC-0043。整数時刻・停止状態期待値を算定し、製品フィルタ・到達/完了の停止境界・受信集計の試験を実施。全拒否/全一致を含む本表の全入力差分への適合判定は[実行記録](#product-execution)の範囲と区別する |

<a id="product-execution"></a>

## 7. 開発中ソースの製品実行記録

2026-10-04、`can.fd.precomputed.v1`と`ethernet.l2.100base-t1.v1`を開発中ソースへ追加し、同一buildで既存profileと実行した。[実行記録JSON](../results/ethernet-media-canfd-2026-10-04.json)にソース・入力・出力のSHA-256、コマンド、合否と検証境界を保存する。v1.1.2向けPRとして公開準備し、tagの作成は未実施。

| 実施範囲 | 結果と証跡 |
| --- | --- |
| FD/T1 fixture | fd.iniとt1.iniのvalidate/run/viewが成功。[製品結果checker](../fixtures/original-network/verify_results.py)でFDのbinding・位相時間・3schema・4metricと、T1の双方向・PHY時刻・媒体schema/metricを独立期待値へ照合 |
| FD時間・入力 | [Rust結合試験](../../../crates/dir-simulator/tests/canfd.rs)9件でvectors.jsonのFD時間/DLC、合成後ceil、BRS、正規化と再結合hash、欠落・未知・重複キー、証跡・速度・型・容量・cross-profile拒否を確認 |
| FD実行と停止 | standard/extended仲裁、同時生成とdrop-newest、容量0、理想同報、フィルタ拒否、EOF/Arrival/受信処理のT等号境界、時刻overflow時の確定prefixとSOF非commitを確認。専用通知codecの型・所有・世代検査を内部単体試験で確認 |
| 100BASE-T1 | [媒体Rust結合試験](../../../crates/dir-simulator/tests/ethernet_media.rs)で専用profileのpolicy・双方向独立・PHY時間を確認。共用pipelineの停止・旧attempt Arrival・Switch試験は[媒体の実行記録](Ethernet媒体拡張検証仕様書.md#product-execution)を参照 |
| CLI・成果物 | 媒体20・FD/T1 2・全example 17の計39入力をvalidate/run。36件の成功出力をviewし、144データファイルのmanifest SHA-256を確認。3件は期待したprepare拒否であり結果公開試験の成功数へ含めない |
| 回帰・viewer | 全Rust275件成功、全JavaScript108件中106件成功・2件skip。FD/媒体、既存Ethernet、既存CAN/GWの3ブラウザscriptが成功。FD詳細にDLC/BRS・二速度・位相bit数・証跡・fidelityを表示し、実績のない受信を生成しない |
| 静的解析と文書 | 42解析ケースと2構成入力、Python文書試験39件、strict traceabilityと生成資料の整合を確認。製品実行とは別系列の証跡として扱う |

FDの位相bit数とevidenceは利用者が供給する。今回の合格は`fidelity=externally-precomputed-phase-bits`・`wire_validation=structural-only`の時間・構造契約に対するもので、内容依存CRC/stuffingの計算や合法波形の証明ではない。公開Registry/Context/Envelope API、全journal予約失敗注入、本書の全改変表・全同一ID競合・全フィルタ組合せ、実機・規格全体適合・大規模性能測定は未実施。
