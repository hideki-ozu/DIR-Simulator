# MAC Merge詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`feat/complete-remaining-capabilities-20261021 @ 082629e1ecb3c966f60a043819c2362ef50e627f`
文書ID：`design-mac-merge`
文書状態：未commit・未公開。選択した静的ideal-clock契約の製品検証・独立レビュー完了。

<a id="merge"></a>

```trace
{"id":"design-mac-merge#merge","stage":"design","requirements":["DIR-REQ-0313"],"upstream":["architecture#mac-merge"],"state":"confirmed","pending":[]}
```

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.1.0` | `2026-10-21` | 同一serializerのMerge TX/RX/verifyと保存契約を追加 |

`runtime/network/mac_merge/`のTX/RX/verify reducersを既存NetworkState/NetworkDeltaに結合する。selected eMAC/pMAC、actual packet、EOF/IPG、伝搬、link epochとfaultを同じFESで所有する。通常transferの最終到着を断片の予定到着で先に予約しない。遅いexpress/HOLDは既に送ったprefixを取り消さず、最初の合法境界から先だけを再計画する。

mCRCは原MACを通じた累積CRCであり、fragmentごとに初期化しない。mPacket headerや中間mCRCを再構成MACへ混ぜない。予約失敗時はstate、queue、packet、counter、recordを一括で公開しない。Verify timerはenqueue時刻でなく実TX完了に結合する。同時刻Respondが期限切れより先に有効となるよう、phase1のprobe/commit stageとその保存予約を明示する。

LLDPはbase mandatory TLV/MSAPの観測とoptional Additional Ethernet Capabilitiesを分ける。peer capability/TTLとRespondの採用は実受信byte、direction、epoch、SOF/EOF/arrivalの証拠へ結合する。checkpointは単独version11、規範FRERとの組合せversion13を選択する。RX assemblyのcounter/byte/CRCを値域検査だけで承認せず、受理した実fragment証拠から再生して比較する。

shaperへは媒体占有とcredit上の送信区間を別に渡す。中間mCRC、継続header、追加IPGも媒体を占有するが、追加overheadで予約classのdata creditを二重に消費しない。TASのHOLD/RELEASE advanceと時計補正による再計画も共通ownerで適用する。gPTPはexpress packetの実SFD直後をcaptureし、Mergeの予定時刻を時計観測に読み替えない。

## 最終の履歴credit・HOLD検証

mandatory-only LLDP撤回、capability省略更新、実fragmentからのRX組立再計算は修正を完了した。各CBS outputの履歴は有限なtyped cause/実wire proofとして保持し、復元時にzero-creditの新しいTSN状態からqueue・実segment・credit・residual・wakeを再計算する。実packet pruning後もproofは保持する。上限はevents 100000、proof octets 67108864であり、超過は原子的に拒否する。保存creditを正本とする任意のcheckpoint anchorは設けない。

HOLDはpActive喪失で自動解除しない。実行例では897224000psにHOLD、1000576000psにLLDP撤回、1199904000psのinactive RELEASEは無効、2001824000psに実V/Rで再検証、3199904000psのactive RELEASEで同じpMACを再開した。expressはHOLD中にも送信できる。

修正後のcoreは19件（8か所のallocation失敗を含む）、製品経路は24件に成功した。9種類の履歴改変を拒否し、HOLD周辺16停止点とlink-reset周辺3停止点で保存継続が一致した。独立レビュアーも凍結した製品test binaryを再実行して24件すべて成功した。最新の実HOLD Viewerは1440px/390pxでinactive HOLD、再検証後HOLD、実RELEASEと逆方向replay、page error/外部requestなしを確認した。

GCL置換とlocal-clock CBS/TASの結合は別の未完了項目として保持する。選択した静的ideal-clock範囲の成功を、その結合成功へ読み替えない。

[最終独立レビュー](../verification/results/remaining-20261021/mac-merge-causal-final-review.md)を参照する。
