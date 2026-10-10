# AC0008 出自・配布人口の限定追補

文書バージョン：`1.1.0`  
対象GitHubバージョン：`main @ b45515644dfc65c205216d564d5bf7ef00280622`  
文書ID：`ac0008-origin-followup`  
状態：既存証跡と固定上流URLの照合。採用承認・作者申告・全静的包含は未確定。

### 更新履歴

| 文書版 | 日付 | push単位の内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-10 | 初回push。保存済み集計と固定上流ソースを別取得して照合し、旧packetを保持した限定追補と再生成checkerを追加 |

## 追補の対象と証拠の境界

これは[旧レビューpacket](../ac0008-review-2026-10-10/README.md)の「情報不足」を日付・対象集合ごとに補う記録である。旧JSON、当時のWSLアクセス拒否・not_run、当時の欠落欄は変更しない。今回の根拠は公開済みb455156の保存資料と固定された上流ファイルであり、Libraryの原JSONを受領・hash照合した記録ではない。新しいRustビルド、binary実行、ガイド生成、全member監査、Release再構築も実施していない。

[機械可読な追補](scoped-reconciliation.json)には旧50行の安定keyと限定的な集合対応を保存した。[固定URL・Git blob・SHA256の受領記録](source-receipts.json)は保存先commitと観測sourceを区別する。公開ソースのbyte/hash照合とPythonでの再計算を実施した。対象は現在のv1.1.4配布物を新規測定した集合ではない。

## 異なる四つの集合を区別する

| 集合 | 対象と確認した範囲 | 確認していない範囲 |
| --- | --- | --- |
| Linux metadata 外部30 packages | bfe0ebedaef7ed814d06d2f90ca0a7dbe453c54a、x86_64-unknown-linux-gnuの[保存graph](https://github.com/hideki-ozu/DIR-Simulator/blob/b45515644dfc65c205216d564d5bf7ef00280622/docs/verification/results/ac0008-vscode-execution-2026-10-10/results/build-link-graph.json) | 実行ファイルへ残った全静的object/member |
| compiler-artifact 外部28 packages | ddfa7bd916212a0b9fedb6adffa9bdb5e83c8401の[保存compiler記録](https://github.com/hideki-ozu/DIR-Simulator/blob/b45515644dfc65c205216d564d5bf7ef00280622/docs/verification/results/ac0008-vscode-execution-2026-10-10/results/build-link-current-ddfa7bd91621/compiler-artifacts.json)。42 records、製品込み29 package IDs | Dead-code removal後の全静的包含。30との差はerrno/libcの2件で、普遍的な非同梱とは断定しない |
| ガイド生成環境17 wheels | [保存Python人口](https://github.com/hideki-ozu/DIR-Simulator/blob/b45515644dfc65c205216d564d5bf7ef00280622/docs/verification/results/ac0008-vscode-execution-2026-10-10/results/build-link-python-population.json)のwheel/hashとinstalled name/version対応。環境再構築なし | 配布同梱依存17件とは扱わない。固定ZIPのwheel memberは0件 |
| 固定ガイド27 assets | 同資料のfixed_artifact_guide_asset_checksを再計算。固定ZIPとMkDocs1.6.1 wheelの記録hashは27件すべて一致 | 残る37 guide membersの再監査とguide8行それぞれの採用承認 |

旧Cargo42行は観測28、metadataだけ2、当該Linux metadataに不在12へ区分する。後者12を全target・全配布から除外したとは書かない。guide8行には実行ファイルの静的リンク判定を流用しない。50行のarchive_membership/deficitsを一括削除せず、元のmember照合と今回の27asset記録を参照する。

## 固定ELFの動的依存

[保存ELF記録](https://github.com/hideki-ozu/DIR-Simulator/blob/b45515644dfc65c205216d564d5bf7ef00280622/docs/verification/results/ac0008-vscode-execution-2026-10-10/results/build-link-elf.json)の対象は0b7b23d8e23fcb1a1491cd13cd42aa5b96a9ab1cの固定ZIP内bin/dir-simulatorである。binaryは11,637,648 bytes、SHA256は`d29b180dd7b46a46a52a18bf62ba3f5628d87e3303efaf398e0bf09c7a3e7129`。interpreterは`/lib64/ld-linux-x86-64.so.2`、DT_NEEDEDは`libgcc_s.so.1`、`libc.so.6`、`ld-linux-x86-64.so.2`の3件。

current_host_resolved_librariesは調査時の解決先であり、歴史的なhost library bytesやZIP同梱を証明しない。全静的object/memberのlinker mapは未取得で、b455156の新規binary検証へ転用しない。

## Snowball v0.3の固定された出典連鎖

[lunr-languagesの固定build tree](https://github.com/MihaiValentin/lunr-languages/tree/f313734d145048be2f3681b756f9bf925aa299a1/build)のbuild/snowball-jsはgitlinkであり、[2011-03-09のfortnightlabs fork](https://github.com/fortnightlabs/snowball-js/commit/f7cdf98e5be76f77f64ecf4cf17acc2d907f6e60)を指す。その[LICENSE](https://github.com/fortnightlabs/snowball-js/blob/f7cdf98e5be76f77f64ecf4cf17acc2d907f6e60/LICENSE)はMPL1.1である。原典[SnowballProgram.js](https://github.com/fortnightlabs/snowball-js/blob/f7cdf98e5be76f77f64ecf4cf17acc2d907f6e60/stemmer/src/SnowballProgram.js)と[Among.js](https://github.com/fortnightlabs/snowball-js/blob/f7cdf98e5be76f77f64ecf4cf17acc2d907f6e60/stemmer/src/Among.js)にはv0.3、2010 Oleg Mazko、Urim/MPLの表示がある。これは表示の確認であり、作者本人の申告ではない。現代の別版のBSD表示を代入しない。

原典function SnowballProgram()と[lunr.stemmer.support.js](https://github.com/MihaiValentin/lunr-languages/blob/f313734d145048be2f3681b756f9bf925aa299a1/lunr.stemmer.support.js)のSnowballProgram: function()の**外側の波括弧を含む本体全体**を抽出し、空白を除去すると双方3,743文字で一致した。正規化後UTF-8 SHA256は`45a3fca22291091557083dde3fcc71f2d308ac19f8772a3d9c141e3c5b563bd3`。関数名・外側構造を含むファイル全体のbyte一致や、実行意味の同値試験ではない。JavaScriptは実行していない。

上流lunr.stemmer.support.jsのSHA256は保存asset記録の`9acfb121aae107828dc54f15cf779ef4a151cdaaade158be705a26e6e96ec618`と一致する。上流比較と固定ZIP/wheelの既存hash対応は別の証拠である。今回ZIPを再展開して照合したとは扱わない。

Amongは原典のprototype helperから比較先のconstructor内instance helperへ配置が変わり、namespace/UMD wrapperも変わっている。SnowballProgramの一致を全体無改変へ広げない。[lunr.ja.js](https://github.com/MihaiValentin/lunr-languages/blob/f313734d145048be2f3681b756f9bf925aa299a1/lunr.ja.js)のstemmerはwordをそのまま返す。[build/build.js](https://github.com/MihaiValentin/lunr-languages/blob/f313734d145048be2f3681b756f9bf925aa299a1/build/build.js)のja entryにtemplate指定はなく、template以外の整形/minify経路がある。日本語実装全体がSnowballから自動生成されたとは断定しない。

元のUrim配布archive、最初のimport履歴、出典連鎖を採用判断に十分とするか、作者本人の申告は未確定である。旧UMDのpinned_commit/license_blob/hash/adapted_wrapper_evidenceは保持し、原典を未発見へ戻さない。正確なimport revision、最終notice/source-listへの統合と採用判断は残す。

## 再生成と未承認事項

旧[ac0008_packet.py](../../../../scripts/verification/ac0008_packet.py)と旧ledger-review-proposals.jsonは歴史packetとしてbyteを保持する。この追補では旧一律欠落欄を上書きせず、[別checker](../../../../scripts/verification/ac0008_origin_followup.py)で旧入力hash・50安定key・限定集合・未承認状態を検査する。旧generatorが初回文言を再生成しても、今回の判断には本追補を併せて参照する。

`python scripts/verification/ac0008_origin_followup.py`でscoped-reconciliation.jsonを再生成し、`--check`で確認する。事前取得した固定ソースをinput-8.txtからinput-13.txtとして保持した場合のみ`--upstream-cache <directory>`で本体比較を再計算できる。checkerは外部通信・原典再取得を行わない。通常のcheckは保存資料と受領記録の整合確認であり、上流ソース再取得とは区別する。

**50行のadoption、40行のOR選択、作者申告は未承認のまま。** rustix CC0の以前のHTTP403は不足として保持し、再取得・別経路・迂回は行わない。完全な静的包含・全AC0008・全仕様適合の合格証明でもなく、main/Release/Siteへ公開反映する判断は行っていない。
