# CAN出力の局所最適化・実測記録（2026-10-08）

文書ID：`can-export-optimization-2026-10-08`

既存の出力処理の2か所を変更した。外部ソートのマージで選択行を深くコピーする処理を所有権の移動に置き換え、canonical JSONの生成では再帰処理を1つの文字列バッファに追記する方式にした。ソート順、JSONのキー順・エスケープ・数値表記、公開インターフェースは維持した。

## 測定結果

同じ生成入力と固定したreleaseバイナリを使い、準備から出力確定までのwall秒をGNU timeで測定した。CPUサンプリングはこの比較計測には併用していない。

| 系列 | 生成要求数 / rho | 変更前の中央値 | 変更後の中央値 | 短縮率 | 最大RSS 前 → 後 |
| --- | ---: | ---: | ---: | ---: | ---: |
| マージ変更のみ | 3,200 / 0.30 | 6.78秒 | 6.39秒 | 5.8% | 100.75 → 102.27 MiB |
| 最終変更 | 16,000 / 0.30 | 36.95秒 | 31.45秒 | 14.9% | 117.56 → 116.16 MiB |
| 最終変更・高負荷 | 3,200 / 0.90 | 4.23秒 | 3.99秒 | 5.7% | 101.61 → 101.30 MiB |
| 最終変更・過負荷 | 3,200 / 1.20 | 3.12秒 | 2.82秒 | 9.6% | 110.29 → 110.30 MiB |

各系列はbinaryごとにwarmup 1回と測定3回を行い、2 binaryを逐次実行してpairごとに順序を反転しました。全32実行が完了し、最終binaryの3条件は24実行です。simulation JSONとrun_id正規化後のevents.csv / summary.csv hashは各系列内で一致し、全実行の4ファイルmanifestを照合しました。短縮率と中央値は生測定値から再計算しています。

最大RSSは測定3回のGNU time値の最大です。RSSはすべての条件で低下したわけではありません。結果の比較では実行ID・作成時刻・バイナリ情報を含むメタデータを除いています。

時間計測後の比較出力はmanifest/hash照合後に削除済みです。完了した16,000件profileの約954 MB出力はローカルに保持し、manifestとファイルサイズを確認しました。optimized profileの照合JSONもmanifestと一致します。

## Gateと入力

baseline/final gateのsource hash map 158件を比較し、差分は `crates/dir-simulator/src/output/disk_sort.rs, crates/dir-simulator/src/output/json.rs, crates/dir-simulator/src/output/tests.rs` の3ファイルだけでした。各baseline/optimized snapshotを対応するgate hashと照合済みです。最終gate 4件はexit 0、Rust testは522 passed / 0 failed / 0 ignoredです。baseline、merge、optimizedの各比較binaryのローカルSHA-256もproducer記録と一致します。

最終ソースで `cargo fmt --all -- --check`、`cargo clippy --locked --workspace --all-targets -- -D warnings`、`cargo test --locked --workspace`、`cargo build --locked --release` が通過しました。JSONの制御文字・Unicode・キー順・整数・浮動小数点の厳密な出力文字列を確認する回帰試験を1件追加しました。

3,200件・16,000件と実100万件の入力生成記録およびpin対象ファイルを収録しました。実100万件入力は7ファイルで、profileのrho 0.30 config hashと一致します。READMEはpin対象外です。入力証跡: [can-export-optimization-2026-10-08/inputs/million/generation.json](can-export-optimization-2026-10-08/inputs/million/generation.json)。

## Profileと限界

WSL2上で `perf record -F 199 -e cpu-clock:u --call-graph dwarf,16384` を使用しました。16,000要求の完走サンプリングは変更前6,798サンプル、変更後5,917サンプルで、いずれもlost sampleは0でした。

変更前は結果出力の準備処理 `output::stream::prepare` がCPUサンプルの71.59%、外部ソートのマージが33.55%、canonical JSON生成が9.39%を含んでいました。変更後のマージは29.31%でした。変更後もJSONのエンコード・デコードと外部ソートが主な調査候補として残ります。これらは子関数を含む割合で重複するため、足し合わせません。短縮の判断にはサンプリングを併用しない比較計測を使います。

実100万件profileは60秒で `profiling_window_limit` により停止し、未完了です。観測RSS最大値は121,745,408 byteです。実行中に生成された未公開spool/outputのinventoryを記録してから削除しました。120秒以内の完走と全体peak RSSは **未検証** です。

profile: [baseline flat](can-export-optimization-2026-10-08/profiles/baseline-perf-flat.txt), [baseline cumulative](can-export-optimization-2026-10-08/profiles/baseline-perf-cumulative.txt), [optimized flat](can-export-optimization-2026-10-08/profiles/optimized-perf-flat.txt), [optimized cumulative](can-export-optimization-2026-10-08/profiles/optimized-perf-cumulative.txt), [実100万件 flat](can-export-optimization-2026-10-08/profiles/actualmillion-profile/perf-flat.txt), [実100万件 cumulative](can-export-optimization-2026-10-08/profiles/actualmillion-profile/perf-cumulative.txt)。16,000件profile: [baseline stdout](can-export-optimization-2026-10-08/profiles/profile-baseline-16000-stdout.json), [baseline stderr](can-export-optimization-2026-10-08/profiles/profile-baseline-16000-stderr.txt), [baseline manifest](can-export-optimization-2026-10-08/profiles/profile-baseline-16000-manifest.json), [optimized stdout](can-export-optimization-2026-10-08/profiles/profile-optimized-16000-stdout.json), [optimized stderr](can-export-optimization-2026-10-08/profiles/profile-optimized-16000-stderr.txt), [optimized manifest](can-export-optimization-2026-10-08/profiles/profile-optimized-16000-manifest.json), [manifest照合JSON](can-export-optimization-2026-10-08/profiles/optimized-profile-output-verification.json)。完了出力はローカル `/tmp/dir-profile-can-2026-10-08/profile-baseline-16000` と `/tmp/dir-profile-can-2026-10-08/profile-optimized-16000` に保持しています。

perfはWSL2上のsampling結果で、sampling overheadがあり、cumulative行は重複します。inline化も含むためsymbol単位の原因帰属は限定的です。profileは次の調査対象を示すもので、無計測の完走時間を予測しません。raw perf.dataと実行binaryはローカルに残し、JSONにpathとSHA-256を記録しています。

比較条件、各試行のwall/RSS、正規化hash、manifest、gate/source hash、入力pin、profileとローカルartifactの記録は[can-export-optimization-2026-10-08.json](can-export-optimization-2026-10-08.json)にあります。support file 265件のbyte数とSHA-256も同JSONに記録しています。

JSONの保存ファイルパスは `support_directory` を基準に解決します。小規模な完走比較の短縮率から100万要求の完走時間は推定していません。
