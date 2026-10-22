# NoCフリット詳細機能仕様書

文書バージョン：`1.2.0`
対象GitHubバージョン：`v1.2.0`
公開状態：v1.2.0収録。実装台帳に記録した契約・検証範囲を参照。

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `1.2.0` | `2026-10-22` | 初版。v1.2.0へ収録し、追加実装の契約・検証範囲を確定 |

文書ID：`spec-noc-flit`
文書状態：実装対応草案。`noc.flit.v1`はプロジェクト定義のデジタルwormholeモデルであり、実機規格全体の適合を主張しない。既存の`noc.xy.v1`は維持する。

<a id="network"></a>

```trace
{"id":"spec-noc-flit#network","stage":"spec","requirements":["DIR-REQ-0291","DIR-REQ-0292","DIR-REQ-0293"],"upstream":["DIR-FUNC-0078"],"state":"confirmed","pending":[]}
```

## 入力と有限資源

単一NED実行owner `dir.noc.Flit`がmodel-config `dir.noc-flit.model` version 1とworkload `dir.noc-flit.workload` version 1を所有する。入出力の型は[型定義](../../../crates/dir-simulator/src/lib/types/noc_flit.rs)を正本とする。

`topology`はmesh/torus・幅・高さ、router番号は`y*width+x`。meshは一辺1、単一routerも許す。torusの両辺は2以上。二点torusの東西・南北は同じ端点でも別の物理link/portとし、router昇順、東・西・南・北の順で存在する有向linkへ0始まりのIDを付ける。

`flit_bytes`は1～4096、`buffer_flits`は1～256、`adaptive_vcs`は1～16。各物理入力portはadaptive VC群に加えて独立escape VC 0を持つ。routerは256以下、全FIFO予約枠は1,048,576以下。`pending_packets`は0～4096で、各送信元の一つの活動streamに加える待ちpacket数を表す。各sourceは一cycleに一flitだけ有限local入力FIFOへ投入する。満杯時は待機する。source待ちpacket数の超過は`injection_rejected`を出し、隠れた無限queueへ積まない。

packetは一意`id`、cycle境界の`at_ps`、source/destination router、実payload配列を持つ。flit数は`max(1,ceil(payload.len/flit_bytes))`、末尾有効byte数を保持する。0 byteはhead-tail制御flitとして実行する。同時requestは`(at_ps,id)`順に正規化する。

`cycle_ps`、link/credit/ejection latency、escape stall閾値は正。latencyは整数cycleでありlink/creditとも最低1 cycle。時刻・乗算overflowは失敗する。ejectionの有限queue数は`ejection_capacity_flits`、serial処理は`ejection_service_cycles`。source=destinationも有限local FIFOとejection queueを通る。

## 実フリットと経路

headは空いている下流VCを原子的に予約し、body/tailは同じVCを使う。上流のtail離脱まで対応付けを保持する。VC所有権はそのVCのtail離脱時に解放し、head到着時には解放しない。物理出力linkは全VCを合わせて一cycle一flit、物理入力portもejectionを含め一cycle一read。異なる出力linkは同時に転送できる。同じ出力linkの候補は保存したround-robin cursorにより決める。

adaptive headは現在graphの最短距離を一つ減らすlinkだけを選ぶ。空きVCとcreditが必要であり、available creditが多いもの、安定link/VC IDの順で候補を決める。headが連続`escape_after_stalls`回grantされなければescape待ちへ移る。以後adaptiveへ戻らない。

escape policyは`spanning_tree_up_down_v1`。各連結成分の最小routerを根とする、link ID順のBFS spanning treeの一意pathを使う。祖先方向から子孫方向へだけ進み、適応用VCとは分離する。escape routeは非最短でもよい。torusでも同じ木を使い、adaptiveは実wrap linkの最短pathを使う。dateline方式を実装したとは主張しない。公平な仲裁とdrainする宛先が進行の前提となる。

link launch時に下流creditを減らしinflight枠を予約する。arrivalでFIFOへ移し、creditを二重減算しない。FIFO pop時にcredit返却tokenを作り、指定latency後にだけ再利用する。各物理入力VCで常に次式を維持する。

`capacity = available credits + inflight reserved slots + FIFO occupancy + pending credit tokens`

ejectionへ移す際にnetwork FIFOの枠を返し、ejectionのserial処理がflitを順番に消費する。全flitの消費後だけ実際に組み立てたpayloadを`packet_delivered`へ記録する。時刻limitでは未到着・待機・処理中のpacketを成功にしない。

## 故障・回復

故障eventは一意`id`、cycle境界時刻、物理`link_id`、`up`を持つ。同じlink/timeの重複更新は拒否する。一方向の故障でもrouting graphでは往復両方向を除外する。物理healthy状態は別々に保存・出力する。非対称有向到達性を提供したとは主張しない。

policyは`topology_epoch_abort_v1`。同時刻の有効な状態変更を一つのepoch遷移とし、既に一flitでも投入した非terminal packetを、影響のないpath上のpacketも含めすべて`topology_epoch_aborted`にする。FIFO、inflight、ejectionの未完成payload、VC予約、旧credit tokenを除去し、creditを空buffer容量へ戻してrouting/treeを再構成する。自動再試行しない。まだ投入していないrequestは保持し、新graphで到達不能なら`unreachable`、回復後の新requestは再び配送できる。既に配送したpacketは保持する。

同時刻の処理順は故障→request admission→arrival/credit返却→宛先service→source投入→仲裁/launch。故障時刻と一致するarrivalは先にabortされる。

## 出力・継続

schema 3、record `dir.noc.FlitRecord` version 1。admission/rejection、VC allocation/release、flit injection/launch/arrival/ejection、credit return、escape entry、topology change、delivery/unreachable/abortを実遷移時刻で出す。packet/flit、epoch、port/VC、payloadを該当時に記録する。idle tickを記録しない。

cooperative controlとcheckpointは有効、metrics v2は未提供。codec `dir.noc.flit.state` version 1は全FIFO、pipeline、credit、ownership、routing/tree、source/ejection、cursor、payload組立、timerとimmutable入力を保存する。破損・不整合はload時に拒否する。詳細は[設計](../../design/NoCフリット詳細設計書.md)、[検証](../../verification/cases/NoCフリット検証仕様書.md)を参照。
