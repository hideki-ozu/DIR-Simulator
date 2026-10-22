# WireHost と共有 MAC serializer

同じ NetworkState/FES、WireHost FIFO と実際の serializer を使う公開例です。RSTP/MSTP などの通常の Wire 制御フレームも、既存の MAC bytes と FCS を保持して送受信します。

- `merge.ini`: ideal full-duplex Clause99。class 7 の Wire gate が開くと、送信中 pMAC を合法な fragment 境界で中断し、検証済み Wire RX 後に同じ pMAC を継続します。
- `abstract.ini`: project abstract preemption。同じ FIFO の Wire EXPRESS を既存 fragment/release/resume-gap 機構へ接続します。IEEE Clause99 の適合性を示す例ではありません。
- `sampled.ini`: 実 sampled 1000BASE-T1 startup を含む 6.012 ms。channel delay 16 samples、attachment origin 12345 ps。Wire は fractional GMII cut で開始し、serializer release 後の PCS decode にも同じ packet ID で結び付きます。

```sh
cargo run --release -- run --config examples/network-composite/wire-serializers/merge.ini --output /tmp/dir-wire-merge
cargo run --release -- run --config examples/network-composite/wire-serializers/abstract.ini --output /tmp/dir-wire-abstract
cargo run --release -- run --config examples/network-composite/wire-serializers/sampled.ini --output /tmp/dir-wire-sampled
```

capability adapter の LLDP は untagged の通常フレームとしてポートの default priority を使い、実際のフレーム長で gate/full-fit と現在の CBS credit を確認します。Verify/Respond は既存の Clause99 内部制御です。

通常の Wire frame は PCP/default priority に従い、TAS full-fit・現在の CBS credit・PAUSE を候補自身の長さで確認します。真の typed MAC Control だけが既存の class shaping exemption を持ちます。Wire が先に選ばれても、admission 成功までは FIFO から取り出しません。TX EOF、IPG release、validated RX を別々に記録し、古い epoch の callback が新しい claim を解放することを防ぎます。

Wire と project abstract preemption の composite は workload の `link_set` をサポートします。送信途中の down は完了済み fragment と実際の部分送信バイト・占有時間を保存し、未完了フレームと resume-gap を中断します。EOF 後の遅い RX は退役させます。既に受信済みの delivery は保持し、残った MAC occupation の退役を `reset_ps` で区別します。同時刻の down/up も順序通りに処理します。queued Data は従来のポリシーで保持・再評価し、Wire は既存の expiry/drop ポリシーに従います。この composite の追加能力は `link_set` に限定され、legacy standalone abstract profile の controls 制限は変更しません。

Clause99 の PAUSE **生成は禁止**です。project abstract preemption の生成だけは、別の明示的 project policy `dir.experimental.project_preemption_pause.v1` を必要とします。既存の `dir.experimental.merge_pause.v1` は生成を許可しません。half-duplex の PAUSE 生成、fragmented gPTP、未知の到着 envelope を持つ Wire/GP producer の Qcc 保証は許可されません。構成要素が別出力にある場合は、その出力自身の capability で判定します。

sampled raw API の `reference_sample=None` は従来の整数 exact-cut 契約を保持します。`Some(s)` は unrendered cursor、status cut、sample ID の一致と exact ceiling notification を要求します。ネットワーク側は source availability と、通知処理前の方向別 permission/credit 証拠も検査します。同じ ceiling 時刻の enqueue、clock/GCL/admin/PAUSE 変更を、過去の fractional instant に遡らせません。credit の再構成は既存の capped/causal accounting を前向きに使用します。

8 ns は安定した全 admission 条件の下での GMII cut 間隔であり、任意の credit zero crossing からの無条件の待ち時間保証ではありません。通知の rounding は 1 ps 未満です。Checkpoint は Wire ID、bytes、physical sample、serializer epoch、owner、credit を frozen-input の実行 prefix と照合します。raw Merge の public restore は検証済み Engine prefix を復元し、private native NetworkState restore は raw を明示的に拒否します。ideal Packet の到着式を raw PHY の受信証明として流用しません。

実行検証は `tests/network_wire_serializers.rs`、raw admission matrix は `tests/phy_ieee_gbt1.rs::raw_sample_reference_actual_delay_matrix_and_rejects_stale_future_late` を参照してください。長い sampled tests は `--ignored` で明示的に実行します。これらの例だけで全 81 capability や N13/T06 の完了を主張しません。

独立 bytes/CRC/physical-grid oracle と実行方法は [verification fixture](../../../docs/verification/fixtures/wire-serializers/README.md) を参照してください。
