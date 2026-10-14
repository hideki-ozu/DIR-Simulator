# Ethernet VLAN受入条件の実測検証記録

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.4`
文書ID：`verification-guide-ethernet-vlan`
文書状態：実施済みの範囲を以下に記録

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-14` | 初版：受入admitの1変数測定、正式新規build・SHA guard、隔離ZIP、旧/fresh負例、実Viewer/siteの証拠を追加 |

## 正式版と実施環境

WSL Ubuntu 24.04 LTS x86_64、Rust 1.85.0、正式source commit `b45515644dfc65c205216d564d5bf7ef00280622`。読者手順の新しいclone、SHA一致guard、locked release build、PATH解決、公開all.ini validateを実行した。SHAを全0にしたguardはerrexitなしでも終了1となり、代替cargo関数は呼ばれなかった。正式Releaseはv1.1.4で、v1.1.5はタグのみのため測定には使わない。

[`measurements.json`](measurements.json)はfreshの全3simulation（model_records、records、summaryを含む）とbinary/source/lock/input SHAを保存する。oldとfreshを順に新規出力へ実行し、各18 CLI操作、計36操作。各条件のリポジトリ入力と隔離ZIPの全simulationが同一だった。ZIPは厳密8memberで全byteを公開入力へ照合した。

## 検査内容と証拠

- `verify_guide_ethernet_vlan.py`：ports[3].admit以外のJSON同一、INI差分はmodel-config名のみ。validate、run/view終了0、clean正式source、manifestの全byte/hash、完全終了・pending 0、タグ・CRC・68byte、source/next hop時刻、即filterのnull予定時刻、全理由和、flow遅延4成分、bitsとutilization、ZIP同一を検査。
- `verify_ethernet_vlan_single_variable_regression.py`：余分field、別Portの変更、比較値不一致、INI終了時刻変更の4負例をCLIアクセス前に拒否。
- [`viewer-browser.json`](viewer-browser.json)：実Chromium、指定freshの9入力hash、終了2000000ps、件数・転送行・tag・受信理由を検査し、公開3 PNGを実HTMLの詳細パネルから撮影。全3画像を開いて実画素を確認済み。合成画像ではない。
- [`site-browser.json`](site-browser.json)：22ページを巡回、全画像ロード、390pxで横overflowなし、日本語「受入」「破棄」で新記事へ到達、pageerror/失敗/外部リクエスト0。新記事のdesktop/top390/測定390画像も別途画素確認。
- `verify_ethernet_vlan_evidence_regression.cjs`：old valid出力が存在していても、両browser checkerが出力指定省略・不存在・選択Viewer欠落の6負例を拒否し、最後に対象を復元。
- `check_ethernet_vlan_document_metadata.py`とmetadata負例：実生成器のsource inventoryと両生成一覧の文書版・単独タグ行を照合。タグ行へcommitを追加した不正形式を拒否して復元。
- MkDocs strict、23 HTMLのローカルリンク・画像・アンカー、全15配布ZIP、nav/browser/生成数、Python文書テスト、適用REQ-0224〜0229と全体strict traceability、生成鮮度を検査。記録数と詳細はPR本文にも記す。

未マージのSoC/AHB/NoC/Ethernet優先キューを含むPR85の公開先端を継承し、非共通ファイル・既存資産・過去履歴を保持する。別担当PR81と保留ローカルcommitは編集しない。原稿・入力・画像・検証の変更で、runtime・認証・workflowは変更しない。PRのexact head CIはpush後に確認しPR本文へ記録する。

## 結果と制約

all/tagged_onlyはSwitch ready611000ps、b受信1220000ps、直列化2copy。untagged_onlyは609000psにingress_frame_typeとしてfiltered、次copyとb receptionなし、直列化1copy。全条件drop0。frame1の有限実習で、membership不一致、タグ変換、flood/multicast、queue overflow、連続負荷、deadline、実機・規格全体の適合は未検証。
