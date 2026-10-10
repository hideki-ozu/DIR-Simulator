# 公開履歴と通知・ソース表示の限定追補

文書バージョン：`1.1.1`  
対象GitHubバージョン：`PR #48 @ d79c036fd20c8ba203190739335b7cf1f4d44c65`  
文書ID：`ac0008-public-origin-notice-review`  
文書状態：公開固定履歴・既存表示の限定確認。本人の全体説明は受領済み、個別由来・全配布適合は未確認

### 更新履歴

| 文書版 | 日付 | push単位の内容 |
| --- | --- | --- |
| 1.1.1 | 2026-10-10 | 本人の全体説明を受領した追補へ接続し、個別の由来・権利条件の残件と区別 |
| 1.1.0 | 2026-10-10 | 六つの公開commit、通知・ソース表示と提案の区別、rustixの限定cfg推論を追加 |

## 公開履歴から観測した上流変更

| 日付UTC | 固定commit | 観測 |
| --- | --- | --- |
| 2011-03-09 | [f7cdf98e5be7](https://github.com/fortnightlabs/snowball-js/commit/f7cdf98e5be76f77f64ecf4cf17acc2d907f6e60) | 公開Git root。commit messageはSVN由来を述べるが、そのSVN履歴は再構成していない |
| 2014-04-20 | [8894a6ebc81f](https://github.com/MihaiValentin/lunr-languages/commit/8894a6ebc81faf6e7e713c0a8f51098ec1ab562b) | 公開Git rootでsupport.jsにOleg Mazko 2010/Urim/MPLの表示とAmongのinstance helperが既にある |
| 2014-07-26 | [504557d1bcfa](https://github.com/MihaiValentin/lunr-languages/commit/504557d1bcfa9f5120eb587c122e5e00c8e2868e) | support.jsへAMD/CommonJS/browser wrapperを追加。returnExports.js参照を含む。exact import revisionは未確定 |
| 2014-07-27 | [3cb5ebfde3c5](https://github.com/MihaiValentin/lunr-languages/commit/3cb5ebfde3c541f84089d06736ef4baccb812bb0) | build/snowball-js gitlinkをf7cdf98へ固定し、.gitmodulesにfortnightlabs URLを追加 |
| 2015-10-29 | [7965e9674a8e](https://github.com/MihaiValentin/lunr-languages/commit/7965e9674a8e5299d9f9e9150b3883acfd13d2c9) | trimmerSupport.generateTrimmerを追加 |
| 2017-04-03 | [4c64ac618e5c](https://github.com/MihaiValentin/lunr-languages/commit/4c64ac618e5c89868c0755761cb6f510d0a74d91) | Lunr 2 token.updateと従来文字列の分岐を追加 |

parent SHA、対象blobと公開patchは[固定記録](public-origin-notice-review.json)に保持する。root以前のUrimアーカイブ／SVNの出自、実際に取り込んだ素材、個別作者の権利申告は未確認。本人によるプロジェクト全体の[作成経緯の説明](作成経緯の本人回答.md)は13:40 UTCに受領済み。Gitの表示を作者による証言へ読み替えない。

既存の[SnowballProgram限定比較](scoped-reconciliation.json)は外側のbraceを含む関数bodyの空白除去後3743文字・SHA256 `45a3fca22291091557083dde3fcc71f2d308ac19f8772a3d9c141e3c5b563bd3`が一致した観測である。ファイル全体のbyte一致、JS実行同値性、全日本語stemmerがSnowball生成という主張ではない。Amongの配置変更、UMD、trimmerは別の変更である。

## 既存表示と追加・提案

| 対象 | 既存／今回の状態 | 残る判断 |
| --- | --- | --- |
| ガイドのlunr-languages行・MPL原文 | 元からある。今回Oleg 2010表示・固定fork・公開変更履歴への短い参照を追加 | MPLの全配布条件達成を意味しない |
| sources.txt／非圧縮JA・supportソース | 元の版固定参照を保持。fork/helper・UMD参照・trimmer変更の限定説明を追記 | 実公開サイトの再確認・供給／保持条件の判断 |
| UMD MIT原文 | 管理文書を変更せず、[原文だけの保存案](proposed-umd-MIT.txt)を別に用意。lunr.stemmer.support.js、lunr.ja.js、lunr.multi.jsのwrapper表示に対応 | exact import revision、公開assetへの統合・適用判断は未承認 |
| MPL由来 | 固定forkのMPL-1.1とソース表示を保持 | modern SnowballのBSDを過去forkへ代用しない |

## MPL条件レビュー

[MozillaのMPL 1.1原文](https://www.mozilla.org/en-US/MPL/1.1/)の各節に対応する限定レビュー。文書の追加で適合完了と判定しない。

| 節 | 既存で確認したこと | 残る対応判断 |
| --- | --- | --- |
| 3.1 | MPL本文・対象ソース参照がある | Covered Codeの変更をMPL sourceとして扱い、ライセンスを渡すことの配布時確認 |
| 3.2 | 非圧縮ソースの参照がある | 同じmedia又は電子的供給機構。電子的な場合の初回12か月／後続6か月の保持と第三者供給の確実性を別に判断 |
| 3.3 | Oleg表示と上流変更日を今回明示 | 変更の説明・日付、Initial Developer由来の顕著な表示を対象ファイル／起源通知と照合 |
| 3.5 | 元headerとMPL本文を保持 | Exhibit A通知の対象ファイルへの付与又は構造上適切な位置、権利通知の確認 |
| 3.6 | ガイドへの原文・source入口がある | executable形式を配る場合の3.1〜3.5と、ソース入手先・方法の顕著な案内。別licenseはsourceの権利を制限しない |

DIR全体をMPLへ変更する提案でも、違反認定でもない。電子的機構の保持条件を全媒体の一律期限へ拡張しない。

## rustix vDSO／CC0の限定cfg観測

rustix 0.38.44の固定上流2ファイルだけを公開URLから読み、bytes/hashを記録した。[linux_raw/mod.rs](https://github.com/bytecodealliance/rustix/blob/acf4a284ea893efa93953e1ff2f2f0ab00be1cdc/src/backend/linux_raw/mod.rs)のvdso/vdso_wrappersは `any(feature="time", feature="process", target_arch="x86")`。保存ddfa7bd compiler記録のrustix lib featuresは alloc/default/fs/libc-extra-traits/rand/std/use-libc-auxv、targetはx86_64-unknown-linux-gnuなので、この組合せのpredicateはfalseである。

[vdso.rs](https://github.com/bytecodealliance/rustix/blob/acf4a284ea893efa93953e1ff2f2f0ab00be1cdc/src/backend/linux_raw/vdso.rs)はLinuxのparse_vdso.cからの翻案とCC0表示を持つ。crate archiveのファイル存在と、今回のcfgでmoduleを選ぶこと、実行ファイルに含むことは別である。ここから全target／過去0b7b23d製品／全静的objectの非包含は結論しない。全rustc引数・link map・新binaryは取得していない。親が確認したarchiveをこのPCで受領したとも記録しない。

CC0本文の既知HTTP403は未解決。別経路の取得や新しい配布物作成はしていない。MIT選択でrustix COPYRIGHT・個別ファイルの別条件は消えない。ソース配布時のvendor／本文収録を自動的に完了扱いしない。

40件のMIT経路承認は[別の現在記録](license-selection-2026-10-10.json)へ記録する。過去の未選択／50件採用未承認記録は変更せず、本人の全体説明待ちは解消し、個別の由来・権利申告、全配布適合の残件を維持する。
