# PSFPガイド検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-psfp`
文書状態：ローカルCLI・Viewer・サイト・追跡構造検証は合格。Project登録は権限不足で未実施

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-15` | 初版：yellow_action1変数比較・正式版CLI・独立ZIP・実Viewerとサイトの検証証跡を追加 |

## 出自・適用範囲・期待値

最新main `e6c5e74c3e44db7b1ce3f3d1caf0b1d628dce7ed`、Release/タグ、全Issue/PR一覧とopen作業、README、TSN仕様/検証仕様、push単位の文書改訂規定を確認した。PR96/97/98はmerged、open PRなし。正式Releaseはv1.1.4、v1.1.5はタグのみ。リポジトリ内のAGENTS.mdと`.agents/skills`は存在しなかった。独立した`docs/ethernet-psfp-guide`と新規作業ツリーをmainから作成し、既存作業ツリーは編集しない。

mainには正式タグ後のNED修正・起動診断等を含むが、本実測は正式v1.1.4 source `b45515644dfc65c205216d564d5bf7ef00280622`を対象にする。WSL Ubuntu 24.04 LTS x86_64・Rust1.85.0の既存clean dev binaryを再利用し、SHA256を照合する。release最適化buildの性能測定ではなく、新しい並行ビルドも行わない。

公開psfp-model/workloadとethtsn NEDを使用し、yellow_actionだけを変更した。期待値は[PSFP契約](../../../specs/models/EthernetTSN詳細機能仕様書.md#policing)から独立計算した。1Gbps・MAC64byteの直列化576000ps、占有672000psより到着576000/1248000/1920000/32576000/96576000ps。tokenはbit×10^12でcommitted 8Mbps/64byte、peak16Mbps/128byte、cap・到着間隔を使う。製品出力やfixtureの同時到着例を期待値の生成元にしない。

DIR-REQ-0252/0253/0254/0255、DIR-FUNC-0063、既存DIR-TEST-0115/0116のうち、単一stream・meter・結果/Viewerの一部シナリオを補足する。要件・仕様・製品コードの変更はなく、全経路検査の合格を仕様全体の意味的充足やIEEE適合と読み替えない。

## 実測とViewer

[measurements.json](measurements.json)は`scripts/verify_guide_ethernet_psfp.py`の生成結果。2条件×validate/run/view×repository/隔離ZIPの12操作がexit0。complete/partial false/events_exhausted/pending0、全manifest file hash、Git commit/clean・binary/source/lock hash・現行6入力hashを照合した。全copy時刻/CRC・MAC長、色・verdict・token全before/after・consumed_bits・受信理由/時刻と集計が独立期待値と一致し、隔離ZIPの全simulation objectも一致した。

両条件で5送信、色green/yellow/red/yellow/green、token消費は1024/512/0/512/1024bitで不変。yellow_action pass/dropでreceived4/2・filtered1/3、copy dropped0。条件間でtokenとwire/時刻が同一であり、yellow dropで返却しないことを確認した。

[viewer-browser.json](viewer-browser.json)に実Chromiumの結果を記録する。明示したfresh CLI出力と測定証拠・現行入力hashを照合し、別runへfallbackしない。1247999psはgreenの最終評価、1248000psの到着でyellow。copyはserialized/drop_reason null、receptionは受信完了またはpsfp_meter_yellow。red消費0、後続yellowの同一token、最終5送信/4または2受信を確認し、390pxの実streamカード3 PNGを撮影した。

## 残る制約・管理

MkDocs1.6.1 strict、27 HTML（404込み）・1791ローカルリンク/画像/アンカー、全19配布ZIP正本一致、nav/browser26ページ一致が合格。実Chromiumで全26ページの画像読込み・390px幅を確認した。第三者notice/hash照合とその既存19テスト、対象0252/0253/0254/0255と全体のtraceability strict（231要件・60機能・274ノード、構造error0/incomplete0）も合格。公開PNG3枚と記事desktop/390px上部・実測箇所の実画素を開き、文字・単位・時刻・受信理由を目視確認した。幅の広い表/コードは既存CSSの横スクロールで読む。

初回の実ブラウザ検証は複合語「色判定」で記事本体が候補に出ず失敗した。本文で用語を独立させても再現した。生成サイト付属の同じLunr/日本語plugin・search_indexを使う独立確認で、tokenizerは「色」「判定」に分割し、当初のindex.search「色判定」は入口のみ、「判定」「破棄」は本記事を返すことを確認した。共有検索コードや設定は変更していない。本文へ読者向けの検索案内を追加した最終稿では「色判定」も本記事を返した。最終checkerは「色判定」「判定」「破棄」の3語すべてで記事到達を必須assertionに戻して検証する。当初の失敗と最終稿の成功を区別し、他の複合語一般に保証を広げない。

固定速度・伝搬/処理0・単一stream・5frameに限定する。SDU境界、stream gate、bypass、複数hop、後段満杯・途中停止、TAS/CBS合成を今回測定しない。full Rust/native HTTP suiteをローカルで再実施したとは主張しない。公開ガイドへ絶対ホストパス・個人情報・管理メモを含めない。

Projectは今回もread:project不足で一覧取得が拒否された。認証/権限を変更したりアクセス拒否を迂回したりせず、Project登録・進行状態更新は未実施とする。Issueを自動closeし得るDone操作はしない。外部エージェントやCodeRabbitへの新規依頼・完了待ちは行わない。
