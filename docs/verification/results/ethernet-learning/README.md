# MAC学習ガイド検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-learning`
文書状態：CLI・実Viewer・サイト検証合格。Project操作は権限不足により未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-15` | 初版：MAC aging1変数比較・正式版実測・独立ZIP・実Viewerと公開サイトの検証証跡を追加 |

## 出自・期待値・適用範囲

最新main `c67e153cc3bcf1f6ac0303d23023ce8ae4a4f533`、Release/タグ、README・仕様・文書運用規約、既存記事、Issue/PRと進行中作業を確認した。PR99はmergedでopen PRなし。正式Releaseはv1.1.4、v1.1.5はタグのみ。リポジトリ内AGENTS.md/.agents/skillsは存在しなかった。新規作業ツリー・`docs/ethernet-learning-guide`をmainから作成し、既存ブランチ/作業ツリーは編集しない。

実測sourceは正式v1.1.4の`b45515644dfc65c205216d564d5bf7ef00280622`。WSL Ubuntu24.04 LTS x86_64・Rust1.85.0の既存clean dev binaryを実SHAと照合して再利用し、新しい並行ビルドは行わない。release最適化buildの速度測定とはせず、mainの後続NED修正を正式版へ含めない。

NEDは公開Ethernet Main.nedを変更せず使用。公開VLAN実習all.jsonからschema4・空static FDB・1 bridge/3 linksのdynamic部分を共通追加し、公開dynamic unicastをMain名前へ揃えて固定のb→a送信を共通追加した。その後の比較変数はdynamic.mac_age_psだけである。期待値は[学習・aging仕様](../../../specs/models/Ethernet動的制御詳細機能仕様書.md#membership)から独立に求めた。MAC68byte、wire608bit、占有704bit、Link1000ps・Switch2000psよりlearn A609000/B5609000、expiry=learn+age、戻りSOF5611000/arrival6220000psを固定期待値とする。

既存DIR-TEST-0109の学習/期限とDIR-TEST-0112の結果/Viewerの本シナリオを補足する。MAC側DIR-REQ-0240/0241/0242/0243・DIR-FUNC-0060と、観測側の既存topology経路を参照し、要件・仕様・製品コードを変更しない。全体strictの合格を規格適合・全仕様の意味的充足へ読み替えない。

## CLI・独立ZIP・Viewer

[measurements.json](measurements.json)は`scripts/verify_guide_ethernet_learning.py`が生成した。2条件×validate/run/view×repository/隔離ZIPの12操作がexit0。complete/partial false/events_exhausted/pending0、全manifest file hash、formal commit/clean・実binary/source/lock hash・現行6入力hashを確認した。各copyの経路/parent・SOF/EOF/arrival/release・CRC、learn/expireの実施/予定時刻とpolicy差分、理由別集計が一致した。隔離ZIPの全simulation objectもrepository入力と一致した。

短期はAが2609000psに失効し戻り方向がa/cへflood、長期はAが10609000psまで有効でaだけへ転送。copy6/5、received2/2、destination_mismatch2/1、copy dropped0。両条件のa到着6220000psは同じである。2回のlearnと2回のexpire後は両方のMAC表が空になり、topology generationは0のまま。

[viewer-browser.json](viewer-browser.json)は実Chromiumの結果。明示したfresh CLI出力と測定証拠・現行入力hashを結び、別runへfallbackしない。初回checkerは折り畳みdetails内のpreをinnerTextで読んで空文字となりJSON parseに失敗した。DOM textContentへ直し、撮影時は実際にdetailsを開く。これは製品の表欠損ではなくcheckerの読取り誤りであり、修正後の検証と区別する。

## 制約・管理

単一Switch・VID10・2frame・競合なしのみ。同source refresh、move、static shadow、表上限、link/STP flush、expiryとlookupの完全同時境界、途中停止、全IEEE状態機械を測定していない。full Rust/native HTTP suiteを今回ローカルで再実施したとは主張しない。管理・執筆記録は公開ガイドの外へ置く。

Projectはread:project不足が継続し一覧取得が拒否された。登録/進行状態更新は未実施とし、認証/権限設定変更・拒否の迂回・Issue自動closeを起こし得るDone操作は行わない。CodeRabbit/他外部エージェントへの新規依頼・完了待ちは行わない。

## 最終ローカル検証

MkDocs strict、28 HTML・1888ローカルリンク/画像/アンカー、全20配布ZIP、第三者ライセンス照合と既存19テスト、全体traceability strict（231要求・60機能、構造エラー/未完了0）を通過した。適用範囲DIR-REQ-0240～0243もstrictで確認した。

実Chromiumで全27 navページの390px表示・画像とコンソールを検査し、日本語検索「学習」「転送」で本記事への到達を確認した。外部リクエスト/ブラウザーエラー0。Viewerはexpiry直前/一致時刻/逆seek、5611000psの表1件/2件、7000000psのcopy6/5と配送、最終空表を検証した。実ViewerのMAC表2枚、記事desktop/390pxと実測表390pxのPNGを目視し、本文と時刻・値が一致し横にはみ出さないことを確認した。

公開前にmain c67e153の不変・open PRなし・PR99の最新コメントを再確認した。自動レビュー結果待ちや外部エージェントへの依頼は行わない。
