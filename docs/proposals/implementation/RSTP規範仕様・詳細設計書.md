# RSTP規範仕様・詳細設計書

文書バージョン：`1.1.0`
対象GitHubバージョン：`v1.1.6`
文書ID：`proposal-implementation-rstp-normative`
文書状態：承認済み限定scopeの現行実装済み（v1.1.6収録）。本書は採用した詳細契約。製品試験の実施範囲・最終判定は別記録。

### 更新履歴

| 版 | 日付 | 内容 |
| --- | --- | --- |
| 1.1.0 | 2026-10-17 | Q-2022本文に基づくcodec、全選択機械、互換処理、独立vectorと残課題を具体化。v1.1.6収録版を確定 |

## 1. 位置付け・出典・scope

[bridge wire設計](RSTP・MVRP-wire実装設計書.md)の既存P-RSTP-R01/F01/S01/A01/D01/T01および02系を補足する。同IDの再宣言やproposal-trace追加はしない。[共通wire契約](../後続拡張契約提案仕様書.md#wire)のroot、packet、FES、原子commitを継承する。

規範はIEEE Std 802.1Q-2022 base edition。照合PDFは2163頁、SHA-256 `1653580bcae81ed4753ff2840b40d67239036078ed550cff23325d6fdd697114`。本書の「PDF頁」は1始まり、本文印刷頁はPDF頁から1を引く。本文/画像/抽出原文はGit管理しない。本書は規範の複製ではなく、実装用に再構成した仕様である。採用版は2022 base edition。別途公開corrigendumを確認できなかった照合範囲は統合設計§3.1を参照。後続改訂の採用と製品適合試験は未実施。

対象は単一CIST、customer bridge、全二重点対点、ForceProtocolVersion=2。MSTI/SPT/provider/L2GP/shared LANは構成として拒否する。**Backup、STP Config/TCN、migrationは含む**。Backupは自己bridgeの別portを広告する情報でも選択される（§13.29.34、PDF573）。旧STPを一律dropする能動RSTPは§14.6（PDF597）のversion2受信条件を満たさない。AdminEdge/AutoEdgeの検出機械も含む。MST/SPTの追加情報を解釈しない互換受信は§3に定義する。標準の全bridge類型への適合は主張しない。

## 2. adapter内設定と内部型

各RSTP adapter instanceは一bridgeの全対象portを所有する。同protocolを複数bridgeに設定でき、同protocol/bridgeの重複所有だけを拒否する。二bridge/triangleはbridgeごとに別adapterとbridge_addressを設定する。以下の`rstp` objectを既存adapter内に必須追加する。他の共通fieldは共通契約に従う。unknown/null/型違いは拒否。設定はprepare後不変（runtime管理変更APIはこの版にない）。標準の推奨defaultとDIRの入力制約を区別する。

| field | 型、default、値域、0 |
| --- | --- |
| `bridge_address` | 必須、12桁ASCII hexのunicast MAC。小文字正準化。全0拒否、prepared領域内一意。既存抽象`bridge_id`から変換しない |
| `bridge_priority` | JSON整数、default32768、0..61440の4096刻み。0可 |
| `force_protocol_version` | JSON整数、default2、2のみ。0による強制STP運用は対象外だがSTP互換は常に有効 |
| `bridge_max_age_s` | JSON整数6..40、default20、0不可 |
| `bridge_forward_delay_s` | JSON整数4..30、default15、0不可 |
| `hello_time_s` | JSON整数、default2、2固定 |
| `migrate_time_s` | JSON整数、default3、3固定 |
| `tx_hold_count` | JSON整数1..10、default6、0不可 |
| `port_parameters` | 必須配列。各共通`ports`にちょうど1entry、追加/重複/欠落拒否 |
| entry.`port` | 共通portsの既存port ID |
| entry.`port_number` | 必須JSON整数1..4095、bridge内一意、0不可 |
| entry.`port_priority` | JSON整数0..240の16刻み、default128、0可 |
| entry.`external_path_cost` | 必須JSON整数1..200000000、0不可。速度から暗黙推定しない |
| entry.`admin_edge` | bool、default false |
| entry.`auto_edge` | bool、default true |
| entry.`auto_isolate` | bool、default false。falseでも§7の規範図のguardを勝手に削除しない |
| entry.`restricted_role` | bool、default false。trueなら当該portをroot候補から除く |
| entry.`restricted_tcn` | bool、default false。trueなら当該portからTCを他portへ伝播しない |

`2*(bridge_forward_delay_s-1)>=bridge_max_age_s>=2*(hello_time_s+1)`を要求。管理timer解像度は1秒（許容0<r<=1）。根拠：§13.18 Tables13-3/13-4（PDF536–537）、§13.25 Table13-5（PDF545）、§13.27（PDF551–559）、§14.2（PDF591–593）。設定の欠落と0を混同しない。BPDU受信timerにはこの管理設定rangeを適用しない。

内部bridge ID=`(priority<<48)|address`、system ID extensionは0。port ID=`(port_priority<<8)|port_number`。u16/u32/u64は符号なし、priority比較は数値昇順。wire timerはu16の1/256秒単位を保持する。タイマーカウンタは整数秒のu32、絶対時刻はpsのchecked u64。cost加算はu64中間値で行いu32を超える場合は`arithmetic_overflow`でcallback全体を失敗させる（DIR安全境界であり標準の飽和規則とは呼ばない）。初期MaxHops=20はCIST内部値として保持するがwireには出さず、設定fieldを追加しない。

## 3. wire codec

### 3.1 外枠と全offset

共通`mac_frame_fcs`を使う。DA=`01:80:c2:00:00:00`、SAは共通config rootの必須`port_identities:[{port,source_mac}]`にある当該portのMAC。source_macはcolon区切り正準unicast、全0不可、port/MAC重複不可。全active RSTP/MVRP ports unionを過不足なく列挙し、この値だけをcontrol TX SAの正本とする。`rstp.bridge_address`はbridge ID用の独立値でありSAの代用をしない。untagged IEEE802.3 Length frame、LLC=`42 42 03`、LengthはLLCとBPDUのみ（padding/FCSを含まない）。この封入はTable8-1（PDF207）/§8.13.3（PDF258）、BPDUは§14.1–14.4（PDF589–595）。802.1Q本文だけからMAC FCSアルゴリズムが新たに規定されるとはしない。共通CRC32 Ethernet仕様を使いFCSはlittle-endian。他の複数octet数値は全てbig-endian。

| frame offset(0始まり) | BPDU offset | 長さbyte | 内容 |
| --- | --- | --- | --- |
| 0 | — | 6 | DA |
| 6 | — | 6 | SA |
| 12 | — | 2 | Length、RST=39、Config=38、TCN=7 |
| 14 | — | 3 | LLC 42 42 03 |
| 17 | 0 | 2 | protocol ID=0000 |
| 19 | 2 | 1 | protocol version、TX RST=2、Config/TCN=0 |
| 20 | 3 | 1 | type、RST=02、Config=00、TCN=80 |
| 21 | 4 | 1 | flags（TCNにはない） |
| 22 | 5 | 8 | root ID |
| 30 | 13 | 4 | external root cost |
| 34 | 17 | 8 | RST regional root/Config bridge ID。RST受信ではdesignated bridgeにも同値を設定 |
| 42 | 25 | 2 | designated port ID |
| 44 | 27 | 2 | MessageAge |
| 46 | 29 | 2 | MaxAge |
| 48 | 31 | 2 | HelloTime |
| 50 | 33 | 2 | ForwardDelay |
| 52 | 35 | 1 | Version1Length。RST TX=0、Config/TCNにはない |
| 53/52/21 | — | 7/8/39 | RST/Config/TCNの最小frame用zero padding |
| 60 | — | 4 | 最小frameのFCS |

BPDU長はRST36、Config35、TCN4。どれも最小frame64byte（DA..FCS）。長い受信PDUはLengthに従ってpaddingと分離する。FCS位置はframe末尾4byteで固定offset60ではない。64未満、物理上限超過、Lengthが実在client byteを超える、Length<3、VLAN tag/EtherType frame、LLC違い、DA違いは共通drop。長いPDUを36byteへ切断してから長さ検査してはならない。RX paddingは値を検査せず、TX paddingは0。SAをbridge IDに同一視しない。

flagsはLSBからTC=0x01、Proposal=0x02、role mask=0x0c（Alternate/Backup=1、Root=2、Designated=3）、Learning=0x10、Forwarding=0x20、Agreement=0x40、TCAck=0x80。RST TXはTCAck=0、RXは無視。ConfigではTC/TCAck以外をTX0/RX無視。CIST role code=0のRSTはwire分類をRSTのまま保持し、CIST Master portを作らない。受信CIST root IDのsystem ID extension非0を拒否しない（§14.4 note1）。未使用bitやVersion1Length非0のRSTを一律malformedにしない。CIST role code=0はrcvInfoのDesignated/Root/Alternate/Backupいずれにも該当せずOtherInfoとなる。

### 3.2 完全検査、互換判別

共通frame/FCS検査の後、次を順に適用する。`bpdu_len=Length-3`。最短4byteのheaderを確認後PID非0はdrop。

1. type00かつ長さ>=35はversionを問わずConfig。
2. type80かつ長さ>=4はversionを問わずTCN。
3. type02/version2かつ長さ>=36はRST。v1LengthはTX0だが§14.5(c)には値の拒否条件がない。
4. type02/version>=3の互換受信は§14.6に基づきversion2共通部分のみ解釈し、>=36を必要とする。MCID、MSTI、SPT、後続byteは使わない。§14.5(d)のMST/SPT bridge用35byte fallbackをこのRST-only profileへ持ち込まない。
5. その他（type02/version0,1、短すぎる既知type、未知type）はcontrol_drop。

1/2の後続octet、3/4の36byte以後は無視する。Config受信のproposal/agreement/learning/forwardingはfalse、roleはDesignatedへ正規化する。TCNはpriority/timerを読まず`rcvdTcn`を立てる専用messageとする。Configは受理したversionを問わず`rcvdSTP=true`、TCNはversion0/1のときだけ`rcvdSTP=true`、RSTは`rcvdRSTP=true`（§13.29.31、PDF570）。正常なMessageAge超過はmalformedではなく、情報寿命0を経てAGEDへ進める。不正frameはstate/timer/TX不変とdrop recordのみ。互換規則の解釈境界は§12にも記録する。

## 4. priority・role選択・helper

根拠：§13.9–13.10（PDF519–523）、§13.28–13.29（PDF560–573）。CIST vectorは`[RootID,ExternalCost,RegionalRootID,InternalCost,DesignatedBridgeID,DesignatedPortID,ReceivingPortID]`。全成分をこの順で辞書式比較し、小さい方が優先。message/port情報の同値判定は末尾receiving portを除く6成分。選出のtie-breakには7成分すべてを使う。

RST/Config受信vector=`[R,C,D,0,D,DP,localPortID]`。root候補は`[R,C+external_path_cost,B,0,D,DP,localPortID]`。self候補=`[B,0,B,0,B,0,0]`。`infoIs=Received`だけを対象とし、designated bridge MACが自bridge MACと同じ候補、およびrestricted_role=trueを除く。最小候補の受信portがrootPortId、selfなら0。rootTimesはselfならBridgeTimes、それ以外は受信portTimesのMessageAgeを1秒加え最寄り整数秒へ丸める。本DIR解釈ではちょうど半秒は上向き、`age_seconds=(age_ticks+256+128)//256`。

各portのdesignatedPriorityはrootPriorityからdesignated bridge=B、designated/receiving port=当該portIDへ置換。regional rootはこのRST-only profileではB。designatedTimes=rootTimes。次の順にselectedRole/updtInfoを求める。

| infoIs/条件 | selectedRole | updtInfo |
| --- | --- | --- |
| Disabled | Disabled | 既存値（次の再選択で再計算） |
| Aged | Designated | true |
| Mine | Designated | portPriority又はportTimesがdesignated値と異なる |
| Receivedかつroot選択元 | Root | false |
| Received、非root、designatedPriorityがportPriorityより良い | Designated | true |
| Received、非root、上記以外、相手が自己bridge別port | Backup | false |
| Received、非root、上記以外 | Alternate | false |

`reselect`を全portで消してから計算し、計算中にreselectがなければ全selected=true。更新でselected=false/reselect=trueになったら安定化loopで再実行する。

| helper | 実行規則 |
| --- | --- |
| `betterOrSame(Received)` | infoIs=Receivedかつmsg vector<=port vector（6成分） |
| `betterOrSame(Mine)` | infoIs=Mineかつdesignated vector<=port vector |
| `recordPriority` | msgPriorityをportPriorityへコピー |
| `recordTimes` | age/maxAge/forwardDelayをmsgからコピー、HelloTimeは受信値によらず2秒 |
| `recordProposal` | Designated messageかつProposalならproposed=true、それ以外は保持 |
| `recordAgreement` | rstpVersion=trueかつp2pかつAgreementならagreed=true/proposing=false、それ以外はagreed=false |
| `recordDispute` | Learning bitがtrueならdisputed=true/agreed=false |
| `setTcFlags` | 正規化したTC/TCAckがtrueならそれぞれrcvdTc/rcvdTcAckをset、falseではclearしない |
| `setSyncTree`, `setReRootTree` | 全portのsync又はreRoot=true |
| `setTcPropTree` | 呼出portがrestricted_tcn=falseなら自身以外のtcProp=true |
| `reRooted(p)` | p以外のrrWhileが全て0 |
| `allTransmitReady(p)` | p.selectedかつ!p.updtInfo |
| `allSynced(p)` | 全portがselectedかつrole=selectedRoleかつ!updtInfo、さらにRoot/Alternateならroot以外全synced、Designatedならp以外全synced。Backupではfalse |
| `newTcWhile` | 既に非0なら保持。0ならsendRSTP時HelloTime+1=3秒およびnewInfo=true、legacy時rootTimes.MaxAge+ForwardDelay |
| `newTcDetected` | 既に非0なら保持。0ならsendRSTP時3秒、legacy時rootTimes.MaxAge+ForwardDelay |
| `updtRcvdInfoWhile` | rounded(MessageAge+1秒)<=MaxAgeなら3*portTimes.HelloTime=6秒、他は0。age/maxAgeそのものは変更しない |

`rcvInfo`はDesignatedでsuperior又は同vector・異timerならSuperior、同vector・同timer・infoIs=ReceivedならRepeated、それ以外のDesignatedならInferior、Root/Alternate/Backupかつmsg>=portならNotDesignated、その他Other。superiorは§13.10の式を文字通り適用する。designated MACとport numberが同じなら同値でもSuperiorとなり、後続Repeated分類には到達しない（§12）。TCNはこの比較を行わずOtherとしてconsumeし、rcvdTcnをTCMへ渡す。

## 5. 実行モデル・初期値・時間

全機械はpure state変換。FES/queue/FDBを直接変更しない。各portにPRX/PPM/BDM/PIM/PRT/PST/TCM/PTX、bridgeにPRS、port timer tickを持つ。各機械state enumと§4の全変数、最新validated message、policy projection、timer generation、tx ordinalをsnapshotに保持する。未定義メモリ値をguardへ使わない。

全bool初期false、timer/txCount=0、priority/Timesはself/BridgeTimes（HelloTime=2秒）で初期化。portEnabledはprepared link/admin状態、operPointToPoint=true、enableBPDUrx/tx=true、rstpVersion=true、stpVersion=false、cist=true、spt/msti/rcvdInternal/infoInternal=false。role/selectedRole=Disabled、infoIs=Disabled。BEGINにより各初期stateのentryを一回実行し、BEGINを下げて安定化する。BEGINは継続した外部入力にしない。

時刻0でinitialize→state安定化→TX intent commit。1秒tickは`n*10^12 ps (n>=1)`に固定、phase0。整数カウンタを飽和減算で1減らす（0は0）。対象はedgeDelayWhile/helloWhen/mdelayWhile/fdWhile/rrWhile/rbWhile/rcvdInfoWhile/tcWhile/tcDetected/txCount。全portの減算を完了後、安定化する。timer代入を受信時刻+N秒のdeadlineへ置き換えない。例えば0.5秒で6を代入した寿命は6.0秒tickで0になる。これは§13.30（PDF574）のtick意味を保存する。非0counterは次のtickを1本共有予約し、tokenで旧tickを無効化する。time overflowはrun失敗。

同時刻は共通phase0 timer/link_set（既存stable順序）→phase1 RX→phase2 TX。複数受信はpacket ID等の共通順序で1件ずつ完全安定化・commit。リンクdownはportEnabled=falseを渡し、PRX/PIM/PTXのglobal guardを優先。リンクupはportEnabled=trueにして選出・migrationから再開し、抽象tree再計算はしない。down時に受信の有効性を先取りしない。

安定化順序はglobal reset、PRX、PPM、BDM、PIM、PRS、PRT、PST、FDB完了投影、TCMをport ID昇順で反復し、その後PTXを実行する。1遷移ごとにentry actionを一回、UCTは直ちに次stateへ進む。PTXのstate変更が生じたら再び前段を安定化し、全機械に有効guardがなくなるまで反復。global transition > role change > 下記表上からのlocal guardをDIR決定順とする。global reset条件が有効な間はreset先stateに留まり、UCTを含む通常出口を評価しない。reset先に既にいてreset作用が全て反映済みなら再entryしない。同じ値の再代入だけで永久にloopしない。明示local self-loopはguardを解消する作用を一回実行する。max_control_steps超過は失敗、部分commitなし。TX frameはPTX entry時点のsnapshotからencodeし、その後の同callback変化で書き換えない。

## 6. Port Information / Role Selection

略記：`U`=selected&&!updtInfo、`D/A/R/M`=infoIs Disabled/Aged/Received/Mine。記載のないguardでは現状態保持、side effectなし。表のentry作用は一回だけ。根拠§13.35–13.36、Figures13-20/21（PDF577–578）。

| 現state/event guard | 遷移先 | entry作用 |
| --- | --- | --- |
| 任意 BEGIN、又は!portEnabledかつinfoIs!=D | DISABLED | rcvdMsg/proposing/proposed/agree/agreed=false、rcvdInfoWhile=0、infoIs=D、reselect=true、selected=false |
| DISABLED、rcvdMsg | DISABLED | 同上 |
| DISABLED、portEnabled | AGED | infoIs=A、reselect=true、selected=false |
| AGED又はCURRENT、selected&&updtInfo | UPDATE | proposing/proposed=false、agreed &= betterOrSame(Mine)、synced &= agreed、portPriority/Times=designated値、updtInfo=false、infoIs=M、newInfo=true |
| UPDATE | CURRENT | 追加作用なし |
| CURRENT、rcvdMsg&&!updtInfo | RECEIVE | rcvInfoを計算（CIST-onlyのrecordMasteredは作用なし） |
| CURRENT、infoIs=R&&rcvdInfoWhile=0&&!updtInfo&&!rcvdMsg | AGED | 上記AGED作用 |
| RECEIVE、Superior | SUPERIOR_DESIGNATED | infoInternal=false、agreed/proposing=false、recordProposal、setTcFlags、agree &= betterOrSame(Received)、recordAgreement、synced &= agreed、recordPriority/Times、updtRcvdInfoWhile、infoIs=R、reselect=true、selected=false、rcvdMsg=false |
| RECEIVE、Repeated | REPEATED_DESIGNATED | infoInternal=false、recordProposal、setTcFlags、recordAgreement、updtRcvdInfoWhile、rcvdMsg=false |
| RECEIVE、Inferior | INFERIOR_DESIGNATED | recordDispute、rcvdMsg=false |
| RECEIVE、NotDesignated | NOT_DESIGNATED | recordAgreement、setTcFlags、rcvdMsg=false |
| RECEIVE、Other | OTHER | rcvdMsg=false |
| 上記5つの受信結果state | CURRENT | 追加作用なし |
| PRS BEGIN | INIT_TREE | 全selectedRole=Disabled |
| INIT_TREE、又はROLE_SELECTIONでany reselect | ROLE_SELECTION | §4のrole選択処理 |

## 7. Receive / Migration / Detection / Transmit

根拠§13.30–13.34、Figures13-15–19（PDF574–576）。timer機械はONE_SECONDでtick待ち、tickでTICKに入り§5減算、UCTでONE_SECONDへ戻りtick=false。

| 機械・現state/guard | 次state・作用 |
| --- | --- |
| PRX任意、BEGIN又は(!portEnabled&&(rcvdBpdu||edgeDelayWhile!=3)) | DISCARD：rcvdBpdu/rcvdRSTP/rcvdSTP/rcvdMsg=false、edgeDelayWhile=3 |
| DISCARD、rcvdBpdu&&portEnabled&&enableBPDUrx | RECEIVE：version flag更新、messageを格納、rcvdMsg=true、operEdge/isolate/rcvdBpdu=false、edgeDelayWhile=3 |
| RECEIVE、rcvdBpdu&&portEnabled&&enableBPDUrx&&!rcvdMsg | RECEIVE：同上。前messageをconsumeする前に上書きしない |
| PPM BEGIN | CHECKING_RSTP：mcheck=false、sendRSTP=true、mdelayWhile=3 |
| CHECKING_RSTP、!portEnabled&&mdelayWhile!=3 | CHECKING_RSTP：同上 |
| CHECKING_RSTP、mdelayWhile=0 | SENSING：rcvdRSTP/rcvdSTP=false |
| SENSING、!portEnabled||mcheck||(!sendRSTP&&rcvdRSTP) | CHECKING_RSTP：同上 |
| SENSING、sendRSTP&&rcvdSTP | SELECTING_STP：sendRSTP=false、mdelayWhile=3 |
| SELECTING_STP、mdelayWhile=0||!portEnabled||mcheck | SENSING：rcvdRSTP/rcvdSTP=false |
| BDM BEGIN&&!AdminEdge | NOT_EDGE：operEdge/isolate=false |
| BDM BEGIN&&AdminEdge | EDGE：operEdge=true、isolate=false |
| NOT_EDGE、(!portEnabled&&AdminEdge)||(edgeDelayWhile=0&&AutoEdge&&sendRSTP&&proposing) | EDGE：同上 |
| NOT_EDGE、edgeDelayWhile=0&&!AdminEdge&&!AutoEdge&&sendRSTP&&proposing&&p2p | ISOLATED：operEdge=false、isolate=true |
| EDGE、((!portEnabled||!AutoEdge)&&!AdminEdge)||!operEdge | NOT_EDGE：上記作用 |
| ISOLATED、AdminEdge||AutoEdge||!isolate||!p2p | NOT_EDGE：上記作用 |
| PTX任意、BEGIN||!portEnabled||!enableBPDUtx | TRANSMIT_INIT：newInfo=true、txCount=0 |
| TRANSMIT_INIT | IDLE：helloWhen=HelloTime |
| IDLE、helloWhen=0 | TRANSMIT_PERIODIC：newInfo |= Designated||(Root&&tcWhile!=0)、UCTでIDLE |
| IDLE、sendRSTP&&newInfo&&txCount<TxHoldCount&&helloWhen!=0 | TRANSMIT_RSTP：newInfo=false、RST intent、txCount++、tcAck=false、UCTでIDLE |
| IDLE、!sendRSTP&&newInfo&&Root&&txCount<TxHoldCount&&helloWhen!=0 | TRANSMIT_TCN：newInfo=false、TCN intent、txCount++、UCTでIDLE |
| IDLE、!sendRSTP&&newInfo&&Designated&&txCount<TxHoldCount&&helloWhen!=0 | TRANSMIT_CONFIG：newInfo=false、Config intent、txCount++、tcAck=false、UCTでIDLE |

PTXのUCT以外はallTransmitReadyが必要。global BEGIN/downを優先してからこのgateを適用する。DisabledのnewInfoだけから送信しない（downなら初期state保持）。RST送信flagsはtcWhile!=0、proposing、role、learning、forwarding、agreeから作り、priority/age/maxAge/forwardDelayはdesignated値、HelloTimeはportTimesから取得する。ConfigはTC/TCAckのみ。TX intentを実queueへcommitできた時点で送信要求が成立し、EOFではFSMを再送信させない。queue満杯はcallback失敗でありtxCountだけ進めない。txCountは1秒tickごとに1減るtoken-bucket型で、Hello周期ごとに0へ戻す方式ではない。

## 8. Port Role Transitions / Port State Transitions

根拠§13.37–13.38、Figures13-22/24/25/26/27（PDF579、581–583）。全PRT guardはBEGINとUCT以外Uを必要とする。`FD=ceil(designatedTimes.ForwardDelay_ticks/256)`、`fD=sendRSTP?HelloTime:FD`、`MA=ceil(designatedTimes.MaxAge_ticks/256)`。role changeはlocal transitionより優先。transient stateは下表の戻りstateへUCTで戻る。

| 入口/guard | state・entry作用 | 戻り |
| --- | --- | --- |
| BEGIN | INIT_PORT：role=Disabled、learn/forward/synced=false、sync/reRoot=true、rrWhile=FD、fdWhile=MA、rbWhile=0 | DISABLE_PORT |
| selectedRole=Disabled&&role!=selectedRole | DISABLE_PORT：role=Disabled、learn/forward=false | !learning&&!forwardingでDISABLED_PORT |
| DISABLED_PORT入口、又はfdWhile!=MA||sync||reRoot||!synced | DISABLED_PORT：fdWhile=MA、synced=true、rrWhile=0、sync/reRoot=false | 保持 |
| selectedRole=Root&&role!=selectedRole、又はROOT_PORTでrrWhile!=FD | ROOT_PORT：role=Root、rrWhile=FD | 保持 |
| ROOT_PORT、disputed | ROOT_DISCARD：rbWhile=3*HelloTime、learn/forward/disputed=false、fdWhile=FD | ROOT_PORT |
| ROOT_PORT、proposed&&!agree | ROOT_PROPOSED：setSyncTree、proposed=false | ROOT_PORT |
| ROOT_PORT、(allSynced&&!agree)||(proposed&&agree) | ROOT_AGREED：proposed/sync=false、agree=true、newInfo=true | ROOT_PORT |
| ROOT_PORT、(agreed&&!synced)||(sync&&synced) | ROOT_SYNCED：synced=true、sync=false | ROOT_PORT |
| ROOT_PORT、!forward&&rbWhile=0&&!reRoot | REROOT：setReRootTree | ROOT_PORT |
| ROOT_PORT、reRoot&&forward | REROOTED：reRoot=false | ROOT_PORT |
| ROOT_PORT、(fdWhile=0||(reRooted&&rbWhile=0))&&!learn | ROOT_LEARN：fdWhile=fD、learn=true | ROOT_PORT |
| ROOT_PORT、同前半guard&&learn&&!forward | ROOT_FORWARD：fdWhile=0、forward=true | ROOT_PORT |
| selectedRole=Designated&&role!=selectedRole | DESIGNATED_PORT：role=Designated、proposing |= !AdminEdge&&!AutoEdge&&AutoIsolate&&p2p | 保持 |
| DESIGNATED_PORT、((sync&&!synced)||(reRoot&&rrWhile!=0)||disputed||isolate)&&!operEdge&&(learn||forward) | DESIGNATED_DISCARD：learn/forward/disputed=false、fdWhile=fD | DESIGNATED_PORT |
| DESIGNATED_PORT、!forward&&!agreed&&!proposing&&!operEdge | DESIGNATED_PROPOSE：proposing=true、edgeDelayWhile=3、newInfo=true | DESIGNATED_PORT |
| DESIGNATED_PORT、allSynced&&(proposed||!agree) | DESIGNATED_AGREE：proposed/sync=false、agree=true、newInfo=true | DESIGNATED_PORT |
| DESIGNATED_PORT、(!learning&&!forwarding&&!synced)||(agreed&&!synced)||(operEdge&&!synced)||(sync&&synced) | DESIGNATED_SYNCED：rrWhile=0、synced=true、sync=false | DESIGNATED_PORT |
| DESIGNATED_PORT、reRoot&&rrWhile=0 | DESIGNATED_RETIRED：reRoot=false | DESIGNATED_PORT |
| DESIGNATED_PORT、(fdWhile=0||agreed||operEdge)&&(rrWhile=0||!reRoot)&&!sync&&!learn&&!isolate | DESIGNATED_LEARN：learn=true、fdWhile=fD | DESIGNATED_PORT |
| DESIGNATED_PORT、同前半guard&&learn&&!forward&&!isolate | DESIGNATED_FORWARD：forward=true、fdWhile=0、agreed=sendRSTP | DESIGNATED_PORT |
| selectedRole∈{Alternate,Backup}&&role!=selectedRole | BLOCK_PORT：role=selectedRole、learn/forward=false | !learning&&!forwardingでALTERNATE_PORT |
| ALTERNATE_PORT入口、又はfdWhile!=fD||sync||reRoot||!synced | ALTERNATE_PORT：fdWhile=fD、synced=true、rrWhile=0、sync/reRoot=false | 保持 |
| ALTERNATE_PORT、proposed&&!agree | ALTERNATE_PROPOSED：setSyncTree、proposed=false | ALTERNATE_PORT |
| ALTERNATE_PORT、(allSynced&&!agree)||(proposed&&agree) | ALTERNATE_AGREED：proposed=false、agree=true、newInfo=true | ALTERNATE_PORT |
| ALTERNATE_PORT、role=Backup&&rbWhile!=2*HelloTime | BACKUP_PORT：rbWhile=2*HelloTime | ALTERNATE_PORT |

PSTはBEGIN→DISCARDING（learning/forwarding=false、学習・転送停止）、DISCARDINGでlearn→LEARNING（learning=true）、LEARNINGで!learn→DISCARDING、LEARNINGでforward→FORWARDING（forwarding=true）、FORWARDINGで!forward→DISCARDING。即時の同期投影で完了し、LEARNINGはデータ学習のみ、転送なし。停止entryはforwarding停止が反映されてから完了と扱う。§8表はCIST-onlyへの特殊化でありMaster機械は作らない。

## 9. Topology Change / FDB

根拠§13.27.28（PDF554）、§13.29.9/10/25（PDF564、568）、§13.39 Figure13-28（PDF585）。`activeRole=Root又はDesignated`。TCMは次の全8状態を持つ。

| 現state/guard | 遷移先・entry作用 |
| --- | --- |
| BEGIN | INACTIVE：fdbFlush=true、tcDetected/tcWhile=0、tcAck=false |
| INACTIVE、learn&&!fdbFlush | LEARNING：rcvdTc/rcvdTcn/rcvdTcAck/tcProp=false |
| LEARNING、!activeRole&&!(learn||learning)&&!(rcvdTc||rcvdTcn||rcvdTcAck||tcProp) | INACTIVE：同上 |
| LEARNING、rcvdTc||rcvdTcn||rcvdTcAck||tcProp | LEARNING：上記flag clear |
| LEARNING、activeRole&&forward&&!operEdge | DETECTED：newTcWhile、setTcPropTree、newTcDetected、newInfo=true、UCT→ACTIVE |
| ACTIVE、!activeRole||operEdge | LEARNING：上記作用 |
| ACTIVE、rcvdTcn | NOTIFIED_TCN：newTcWhile、UCT→NOTIFIED_TC |
| ACTIVE、rcvdTc | NOTIFIED_TC：rcvdTcn/rcvdTc=false、DesignatedならtcAck=true、setTcPropTree、UCT→ACTIVE |
| ACTIVE、tcProp&&!operEdge | PROPAGATING：newTcWhile、fdbFlush=true、tcProp=false、UCT→ACTIVE |
| ACTIVE、rcvdTcAck | ACKNOWLEDGED：tcWhile=0、rcvdTcAck=false、UCT→ACTIVE |

固定登録maskはprepared static member SとMVRP fixed登録を共通coordinatorで正規化したread-only projectionとし、RSTP独自設定へ重複しない。[共通bridge設計のFDB結合](RSTP・MVRP-wire実装設計書.md)を参照する。FDB完了はpure plannerで当該portを学習先に持つdynamic entryから対象keyを列挙し、delta preflight成功後に削除する。**operEdgeなら削除0**。非edge portにfixed registrationが1つでもあるVIDのみ対象とし、static FDBは保持する。MVRPだけで動的登録されたVIDのflushはMVRP/各MRP機械の責務。tcDetectedの変化を同callbackのprotocol間coordinatorへ渡し、MVRP状態更新も同じcommitに含める。FDB作用が完了したsnapshotでfdbFlush=falseにして機械を継続する。ForceProtocolVersion=2固定なのでSTP互換送信中でも即時flushであり、sendRSTP=falseをrapid ageingモードへ読み替えない。

## 10. DIR結合と観測

`decode`は全frame検査後、Config/Rst/Tcnの正規化messageを返す。`step`は全portのsnapshotをコピーし、§5の順で状態変換し、`ProtocolDelta`へstate、policy、FDB、timer、TC通知、TX intents、transition journalを集約する。`encode`は§3のwireへ変換する。適合しないeventを無言で省略しない。

内部PDUは送信adapter IDを受信ownerとして再使用せず、peer ingressのprotocol/bridge所有adapterへdispatchする。RSTP control TXは共通実FIFO/帯域/EOF/IFG/FESを利用する。data forwarding=falseでもBPDUを送受信する。現在送信中のdataはpreemptせず、次の選択時control優先。TC/role/stateの可視変更で共通policy epochを一回進め、queued dataはcommit後policyで再判定する。開始済dataの取り扱いは共通契約を継承し、ここで中央treeやconvergence_psを導入しない。

journalは`machine,port,from,to,guard,changed_variables`、全snapshotは各機械state/§4–9変数を含む。`guard`は上表の行を識別できる固定コード（例`PIM_CURRENT_EXPIRE`）。同一callback内transient stateも記録し最終role/stateだけへ圧縮しない。外部packetはexternal_injection、内部queue TXはその原因eventとordinalを保持する。capacity/token/checked arithmetic/FES/queue/record失敗では全deltaを棄却する。

## 11. 検証vectorとcoverage

[独立fixture](../fixtures/rstp-standard-v1.json)はschema_version=1、status=design_vectors_not_product_tests、test_id=P-RSTP-T01。V101以降を本書に割当。`wire_decode`は正常FCS付きframe、`wire_reject`は到着時不正frame、`fsm_unit`は既存機械の純状態刺激、`calculation`は比較/時間算術、`scenario`は収束の設計期待である。純状態刺激や収束の期待値を実製品動作と呼ばない。

FSM unitの`before`は記載した値を§5の初期snapshotへoverlayし、`machine`だけを一遷移（又は明示したUCT鎖）評価する。全bridgeを安定化した期待ではない。scenarioは別途実装後の全adapter/FES試験を要求する。wireのCRCは独立Python標準ライブラリで再計算し、別のbit単位CRCでも照合する。送受信実装同士のroundtripだけをoracleとしない。

| 機械 | 選択状態数 | 本書のevent/guard網羅箇所 |
| --- | --- | --- |
| PTI | 2 | §5、§7 tick/UCT/zero |
| PRX | 2 | §7 BEGIN/down/valid/consume待ち |
| PPM | 3 | §7 BEGIN/down/mcheck/expiry/STP/RSTP |
| BDM | 3 | §7 BEGIN/edge検出/受信でedge解除/isolate |
| PTX | 6 | §7 BEGIN/down/disable/hello/hold/Config/TCN/RST |
| PIM | 10 | §6 BEGIN/down/aged/update/RX五分類/expiry |
| PRS | 2 | §4、§6 BEGIN/reselect |
| PRT | 25 | §8 BEGIN/5role選択/全local guard/UCT |
| PST | 3 | §8 BEGIN/learn/forward/停止 |
| TCM | 8 | §9 BEGIN/learn/TCN/TC/prop/ack/role/edge |

表は設計列挙のcoverageであり、全transitionを製品実行済みという意味ではない。fixtureにも機械state inventoryとguard_transition_casesを保存する。166 vectorsには64個のstate entry作用、45個のone-guard遷移を含む。全guardの論理組合せを尽くした製品試験とはしない。boundaryは1/256秒、tick同時RX、TX hold解放、message age等値、overflow、自己sender、tie、リンク復旧を含む。製品の全遷移coverage、二bridge/triangle実行、queue/FDB原子性、性能、全規範適合は未実施。

## 12. 採用版解釈・実装gate

1. **同senderの等値再受信**：§13.10 PDF521のsuperior式を文字通り、§13.29.12 PDF565の順に評価する。同sender・同値もSuperior。RepeatedDesignatedの状態・作用は規範表を保持するが、本CIST profileの通常入力からは到達不能とする。非等値guardを独自追加しない。定期受信はmessageを一度消費し、reselectを一度消して安定化する。最終priority/roleが同じならrole change入口は再発火しない。fixtureにこの有限性と強制entry単体を分けて収録した。
2. **RST-onlyによる未来版type02**：§14.5(c)と§14.6 PDF597を合わせ、version>=2をversion2共通部分へ正規化する。最短36byte。MST/SPT bridge用の35byte fallbackを使わない。このprofileはMCIDを持つMST bridgeを称さない。
3. **時間の量子化**：MessageAge+1秒の最寄り丸めはhalf-upをDIR決定規則として固定する。受信MaxAge/ForwardDelay等のwire値は1/256秒を保持し、整数秒counterへ代入するときだけ上向きに量子化する `ceil(ticks/256)`。元のTimes値や再送wire値は丸めない。`tcWhile=MaxAge+ForwardDelay`では和を作ってから一度ceilする。小数秒をrejectせず、unsigned比較に変換誤差を入れない。これは§13.25の整数counterと§14.2.8のwire粒度を結ぶDIR実装規則であり、標準がhalf tie/量子化方法を明記しているという主張ではない。
4. **CIST role code=0**：type02/version2/36byte以上は§14.5(c)（PDF596）に従いRSTと分類する。§14.2.9 NOTE（PDF592）のConfiguration識別という説明を、BPDU typeを変更する命令とは扱わない。本文PDF80 footnote10はNotesを実装要求ではない参考情報と位置付けているため、規範本文の分類・flag処理を優先する。したがってrcvdRSTP=true、rcvdSTPはこの受信ではsetしない、bit0x80は§14.4(g)に従い無視。role code=0はCISTの選択roleには投影せず、§13.29.12の列挙外としてOtherInfoにする。Configへ再分類する特例を追加しない。
5. **安全境界**：cost overflowはcallback原子失敗、FES順序は共通phase、state安定化の複数有効guardは表順。これらは明示したDIR選択であり、別実装と中間transition順が完全一致するとの主張ではない。

未決定の実装判断は0。本文照合済みのこのdraftは設計の受け渡し資料であり、製品実装・全guard組合せの試験・公式規範適合判定・corrigenda採用は未実施。既存「本文未取得」阻害は解消したが、revisionの実装登録とprepare拒否解除には別の実装・検証が必要である。

2026-10-17採用作業記録：本書のP-IDを[正式採用仕様](../../specs/後続拡張採用詳細機能仕様書.md)へ接続した。[現行API・scope](../../guide/reference/後続10分野の現行実装と利用境界.md)と[製品検証記録](../../verification/results/extension-product-2026-10-17.md)を参照。設計vectorと過去のdesign-only報告は履歴のまま保持し、製品合格とは区別する。文書版・履歴は本pushの改訂として確定した。
