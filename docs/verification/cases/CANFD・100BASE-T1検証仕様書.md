# CANFD・100BASE-T1検証仕様書

文書バージョン：`0.1.0`
対象GitHubバージョン：`未リリース（main @ 7738b55）`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初版草案。CAN FD・100BASE-T1の実装契約と独立解析fixture |

文書ID：`verification-original-network`

状態：仕様・設計の決定事項を記述する草案。製品シミュレータは未実装であり、解析fixtureの合格を製品試験合格とみなさない。

| 項目 | 入力と検証境界 |
| --- | --- |
| 解析 | [vectors.json](../fixtures/original-network/vectors.json)、[verify_expectations.py](../fixtures/original-network/verify_expectations.py)。`python3 docs/verification/fixtures/original-network/verify_expectations.py`で42解析ケースと2構成入力を確認する |
| FD構成 | [fd.ini](../fixtures/original-network/fd.ini)、[Main.ned](../fixtures/original-network/models/fd/Main.ned)、[fd.model.json](../fixtures/original-network/fd.model.json)、[fd.workload.json](../fixtures/original-network/fd.workload.json)。64byte synthetic位相長N40/D600、公称500kbps/データ2Mbps。EOF380000000ps、release386000000ps |
| T1構成 | [t1.ini](../fixtures/original-network/t1.ini)、[t1.model.json](../fixtures/original-network/t1.model.json)、[t1.workload.json](../fixtures/original-network/t1.workload.json)。既存ethernet-media/modelsのMain/Types.nedを参照する |
| 静的検査 | JSON重複キー・対象構造・profile・速度・wire binding・計算結果とINIのfile参照を確認。NEDはファイル存在を確認するだけで、parserやruntime試験ではない |
| 製品試験 | 実装後に`dir-simulator run --config docs/verification/fixtures/original-network/fd.ini --output /tmp/dir-fd`、T1はt1.iniと別出力へ置換してschema2結果を照合する。全時刻・ID・状態は完全一致。内部journal、dispatcher順、診断の確認は製品実装後 |

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
