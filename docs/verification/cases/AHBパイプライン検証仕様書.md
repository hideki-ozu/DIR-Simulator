# AHBパイプライン検証仕様書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
公開版：未公開。タグ・Releaseは作成していない。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | Full AHB2 pipeline・byte RAM・応答・停止・checkpointを公開prepare/run経路で検証。初版予定値 |

文書ID：`verification-ahb-pipeline`

文書状態：対象commitは基点であり、本追加は未commit。未公開の製品実行検証。下記の実行範囲はデジタルtransaction/pipelineモデルに限定する。

<a id="dir-test-0134"></a>

```trace
{
  "id": "DIR-TEST-0134",
  "stage": "verification",
  "requirements": ["DIR-REQ-0286"],
  "upstream": ["design-ahb-pipeline#pipeline"],
  "state": "confirmed",
  "pending": []
}
```

対象は[詳細機能仕様](../../specs/models/AHBパイプライン詳細機能仕様書.md#pipeline)、[詳細設計](../../design/AHBパイプライン詳細設計書.md#pipeline)、[実行例](../../../examples/ahb-pipeline/scenario.ini)。DIR-REQ-0286・DIR-FUNC-0076を対象に、新しい`ahb.pipeline.v1`でアドレス/data重畳、burst、lock、SPLIT/RETRYを追加検証する。旧profileをFull AHB2相当に読み替えない。Full AHB2の規格全条項、pin/analog適合、AHB-LiteのSPLIT/RETRY対応、既存SoC/NoCのbyte storage実装を証明するケースではない。

| 製品試験 | 独立期待値と判定 |
| --- | --- |
| `address_and_data_overlap_with_distinct_owners_and_real_read_after_write` | t1でaddress beat1とdata beat0が重なる。t2でaddress owner=high、data owner=low。中断再開はSINGLE/NONSEQ。後続read値は11/22/33/44の各4byte。 |
| `hready_wait_holds_following_address_and_current_write_data` | wait2のt1/t2はHREADY=falseでaddress beat1とdata beat0の91を保持。t3だけbyte commit/次address受理。2beat目の完了はt6。 |
| `every_wrap_length_and_narrow_hsize_transfer_uses_correct_byte_lanes` | WRAP4/8/16をそれぞれ末尾wordから開始し、最初のaddress後に0から順に折り返す。width8・size2・開始2ではlane2/3、次beatではlane4/5だけ更新し、byte0/1/6/7を保持。 |
| `locked_burst_excludes_higher_priority_contender_through_final_data` | locked burstは最初にHLOCK setupのIDLEを1cycle置く。高priority contenderは最後のlocked data完了までaddressを受理されず、最終data cycleのaddressはIDLE。 |
| `retry_reissues_without_mask_and_split_allows_lower_until_actual_hsplit` | 非OKAYはHREADY=false/trueの2cycle。先行beatはattempt0/1の2回addressを受理するがbyte commitは成功attemptの1回だけ。RETRYはmaskなし・通常priority維持。SPLITは該当masterだけmaskし、低priorityが進み、実際のHSPLITの次cycle以後に再受理。 |
| `locked_split_grants_only_dummy_idle_and_locked_retry_retains_owner` | locked SPLIT中はHSPLITまで全実managerのaddressを禁止し、dummy/default IDLEだけを許可する。locked RETRYはownerを保持。いずれもlock成功完了後だけcontenderが進む。 |
| `error_is_two_cycle_and_cancels_following_address_without_memory_mutation` | wait1の後、ERRORはt2(false)/t3(true)。後続addressを取消し、writeは1byteも更新しない。readで初期値1/2/3/4を確認。 |
| `rejects_bad_alignment_1k_boundary_burst_shape_and_unsafe_retry_target` | unaligned address、1KiB crossing、WRAP4の3beat、複数managerが同じRETRY targetへアクセスする構成をprepareで拒否。unmappedアクセスは2cycle ERROR。 |
| `mid_stall_and_split_checkpoint_resume_preserve_slots_bytes_masks_and_attempts` | 公開RunSessionの3/6/9step後にpauseし、wait slot・異なるaddress/data owner・split mask/attemptをsave/restore。継続後の全cycle recordとmemory recordが一括実行と完全一致。 |
| `horizon_and_stop_never_commit_pending_data_phase` | H=3とwait4でpending writeのbyte commitは0件。実際のStopでも0件を維持。H=0のcycle recordは空。 |

実行コマンド：

```sh
CARGO_TARGET_DIR=/tmp/dir-remaining-implementation-target /home/hideki/.cargo/bin/cargo test --locked -p dir-simulator --test ahb_pipeline --test soc
```

実測：新AHB pipeline試験10件、旧SoC/AHB/NoC回帰試験20件が成功。既存試験の期待値・入力・checkerは書き換えていない。

公式IHI0011AのPDFを取得して§3.4、3.6、3.9、3.11、3.12を確認し、locked SPLIT/RETRYはArmのDDI0170A arbiter状態説明も照合した。試験期待値はRTL/製品実行から逆生成せず、上表の手計算address/cycle/byte値を使用した。規格全条項の適合証明、独立RTLとの比較、性能測定は未実施。

公開CLIを現行buildで実行し、`/tmp/dir-remaining-implementation-20261021-evidence/ahb-pipeline-final-run`へ正常結果を保存した。実測は53 callback/cycle、55 model records、address/data同時提示22cycle、実write commit9beat。SPLIT応答はt3/4、HSPLITはt9、RETRY応答はt21/22。既存Node `controlled-model.js` parserが実結果schema3を受理し、全addressの十進文字列とHMASTER tag1–15を検査した。製品viewコマンドで`/tmp/dir-remaining-implementation-20261021-evidence/ahb-pipeline-final-viewer.html`を生成した。ブラウザー対話・画面撮影は未実施。

family4ファイルの`rustfmt --check`は成功。`cargo clippy --locked -p dir-simulator --lib --test ahb_pipeline -- -D warnings`は別作業中の`runtime/can_protocol.rs:240`のneedless_range_loopと`:1119`のnonminimal_boolで失敗し、AHBファイルの診断はなかった。全体lint成功とは扱わず、親作業の最終検査へ引き継ぐ。正式なアーキテクチャ工程`architecture#ahb-pipeline`は親作業で本仕様・設計と接続する。
