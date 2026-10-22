# CANFD-ISO詳細機能仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`spec-canfd-iso`
文書状態：実装ブランチ。未公開。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 初版。内容からのISO FD wire生成、decoder、二速度時間、版付き結果と独立vector照合 |

<a id="codec"></a>

## 入力と動作

```trace
{"id":"spec-canfd-iso#codec","stage":"spec","requirements":["DIR-REQ-0174","DIR-REQ-0175","DIR-REQ-0176","DIR-REQ-0177","DIR-REQ-0178","DIR-REQ-0179","DIR-REQ-0180","DIR-REQ-0181"],"upstream":["DIR-FUNC-0042"],"state":"confirmed","pending":[]}
```

`can.fd.iso.v1`はISO 11898-1:2015 edition 2の理想ACK済みデータフレームを内容から生成する。従来の[can.fd.precomputed.v1](CANFD・100BASE-T1詳細機能仕様書.md#canfd)とは別に選択する。既存profileの外部位相bit数・binding契約は保持する。

| 項目 | 契約 |
| --- | --- |
| 構成 | 既存`dir.canfd.ControllerV1`、`dir.canfd.BusV1`、固定遅延channel。単一Bus、Controller 2個以上。INIとBusの`profile`をともに`can.fd.iso.v1`へ設定する |
| model-config | 必須rootは`{"schema_version":1,"profile":"can.fd.iso.v1"}`。Bus profileとの不一致を準備時に拒否する |
| workload | schema 2、generatorは`id,kind,node,times_ps,frame`。kindは`can.fd.iso.explicit.v1`。時刻・queue・filter・処理遅延・仲裁は従来FD契約と同じ |
| frame | 必須キーは`format,id,data,brs,esi`のみ。formatはstandard/extended、IDは11/29bit、dataは偶数桁hex、長さは0～8/12/16/20/24/32/48/64 byte。brs/esiはbool。wire・外部bit数の指定を拒否する |
| ESI | 明示したビットを生成する。TEC/RECによる自動設定ではなく、当該profileのエラー状態遷移は規定しない |
| 生成 | SOF～EOF、base/extended arbitration、FDF/res/BRS/ESI/DLC/payload、dynamic stuffing、Gray符号化stuff countとeven parity、CRC17（16byte以下）/CRC21（20byte以上）、fixed stuffing、CRC delimiter、ideal ACK、ACK delimiter、EOF |
| CRC | dynamic stuff bitとstuff-count/parityを含む。fixed stuff bitを含めない。初期値はCRC最上位bitのみ1。最後のDLC/data bitの直後にはdynamic stuff bitを挿入せず、CRC field先頭のfixed stuff bitへ進む |
| 時間 | BRSとCRC delimiterはsample pointで速度を切り替える。両境界を合計するとnominal 1bit＋data 1bitとなる。全体の有効位相数N/Dを生成wireから算出し、`ceil(10^12*(N*Rd+D*Rn)/(Rn*Rd))`を一回だけ丸める。intermissionはnominal 3bitを加えて別に計算する |
| decoder | `runtime::canfd::codec::decode`は期待frameを渡さず、bit列からID/format/BRS/ESI/dataを復元し、stuff/count/parity/CRC/reserved/delimiter/ACK/EOF/長さを検証する。入力範囲は完全な理想ACK frame一個 |
| 停止 | 観測期間は[0,T)。未到達EOF・release・受信を実績化しない。frame定義recordとrequest/receptionの実績を区別する |

## 結果とViewer

schema 2の`dir.canfd.frame` version 2へ、内容・DLC・ESI、N/Dと速度、`wire_bits`（SOF～EOFの01文字列）、CRC幅/値、stuff count/code、dynamic/fixed stuff位置、BRS/CRC delimiter位置、採用規格版を保存する。request/receptionは従来のversion 1。`fidelity=content-derived-iso-can-fd-2015`、`wire_validation=crc-and-stuffing`とする。

Viewerはframe内容からwireとCRC/stuff情報を再計算し、不一致を拒否する。詳細へESI・規格版・CRC・stuff数・bit列を表示する。状態の再生には実績milestoneだけを使う。

## 実行例と検証境界

```sh
cargo run --locked -- run --config examples/canfd/iso.ini --output /tmp/canfd-iso
cargo run --locked -- view --input /tmp/canfd-iso/results.json --output /tmp/canfd-iso.html
```

[設計](../../design/CANFD-ISO詳細設計書.md)と[製品検証](../../verification/cases/CANFD-ISO検証仕様書.md)へ接続する。独立実装128vectorとの一致は、この理想frame codecの証拠である。実controller認証、ISO 16845全試験、現行2024版全条項、error confinement・再送・CC混在bus・PHY波形の適合を示さない。これらの追加実装は別の受入項目として追跡する。
