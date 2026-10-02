# NED詳細機能仕様書

文書バージョン：`0.1.1`
対象GitHubバージョン：`v0.1`

### 更新履歴

| 文書バージョン | 更新日 | 更新内容 |
| --- | --- | --- |
| `0.1.1` | `2026-10-03` | v0.1公開に合わせ、文書版を0.1.1へ統一し対象タグを確定 |
| `0.1.0` | `2026-10-01` | 作業内容を集約：初期版の具体的な入力・正常・異常時契約を確定。字句・構文・値解決と登録照合の決定的契約を補完。GW・AXI・Ethernetの登録型・capabilityとprofile別構成検証へ接続。CSMA/CD半二重・1000BASE-T1の媒体別profileと互換境界を追加 |

文書ID：`spec-ned`

文書状態：仕様確定。実装・実行試験は未実施。

## 1. 収集・文法・型解決

[読み込み](../機能仕様書.md#dir-func-0002)、[階層展開](../機能仕様書.md#dir-func-0003)、[型解決](../機能仕様書.md#dir-func-0004)の契約を具体化する。参照版は[公式 OMNeT++ 6.4 タグの NED 言語説明](https://github.com/omnetpp/omnetpp/blob/omnetpp-6.4.0/doc/src/manual/ch-ned-lang.tex)。以下は独自実装の採用範囲であり、OMNeT++全体への互換性宣言ではない。

[構造定義の親要件](../要件定義書.md#dir-req-0005)は、ファイルの読込成功に加え、選択networkの各配置、型・実装対応、接続経路、型の基準値を設定解決へ渡せる状態までを求める。本書の子要件に対応する規則は、宣言の解釈から展開・登録照合・結線検証までを分担する。入力はNEDファイル集合、選択networkと固定Registryであり、出力は元位置を保持した型宣言・展開構造・接続識別である。確定パラメータの採用は[値解決](設定詳細機能仕様書.md#values)、実行オブジェクトの生成は[ライフサイクル](診断詳細機能仕様書.md#lifecycle)へ引き継ぐ。

現行の適用例はClassical CANの`can.cc.ideal.v1`とする。後段の追加モデルへの適用は[将来profileの登録契約](拡張モデル共通詳細機能仕様書.md#selection)を示すもので、0.1.0の提供対象と実装完了を示すものではない。

<a id="syntax"></a>

```trace
{
  "id": "spec-ned#syntax",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0005",
    "DIR-REQ-0015",
    "DIR-REQ-0016",
    "DIR-REQ-0017",
    "DIR-REQ-0018"
  ],
  "upstream": [
    "DIR-FUNC-0002",
    "DIR-FUNC-0003",
    "DIR-FUNC-0004"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定規則 | 受理例／拒否例 |
| --- | --- | --- |
| ファイル | UTF-8、任意の先頭BOM、LF/CRLF。拡張子が小文字 `.ned` の通常ファイルを全探索ルート配下から再帰収集する。ルートは実在ディレクトリ必須。ルート自身を含むシンボリックリンクを拒否し、配下で遭遇したシンボリックリンクも種類を問わず診断する。重複・包含するルートを拒否する。ルート指定順、各ルート内相対パスのUTF-8バイト昇順に読み込む。隠しディレクトリも対象。非NED通常ファイルは無視する | 空のルート集合、読取不能、壊れたUTF-8は入力失敗 |
| 字句 | 識別子はASCII `[A-Za-z_][A-Za-z0-9_]*`、大文字小文字を区別。予約語を識別子位置に検出した場合は構文エラーとして診断する。空白と `//` 行コメント、非入れ子 `/* ... */` コメントを受理する。文字列と数値は[設定仕様のリテラル](設定詳細機能仕様書.md#values)を用いる。字句の詳細は下表で規定する | `node_1` は受理、`node-1` は拒否 |
| package | ファイル先頭に `package a.b;` を必須とし、探索ルートに対する親ディレクトリ `a/b` と一致させる。空packageとルート直下のNEDファイルは未対応として拒否する。package宣言は一回。ファイル名と型名が異なる宣言も受理する | `models/demo/Main.ned` とルート `models`、`package demo;` は受理 |
| 宣言 | package後に一つ以上の `simple Name {...}`、`module Name {...}`、`network Name {...}`、`channel Name {...}`。型本体は `parameters:`、`gates:`、`submodules:`、`connections:` の順で各節最大一回、不要節省略可。simpleは前二節、channelはparametersだけ、module/networkは全節を使用可。空本体も構文としては受理し、後段で必須条件を検証する | `simple X {}` は構文受理後、未登録実装なら拒否 |
| パラメータ | `型 名前 {パラメータ属性} [= default(リテラル)] ;`。型は `int`、`double`、`bool`、`string`。省略値は必須パラメータを意味する。型内の同名パラメータは重複エラー。属性は後述の配置規則に限り、値指定より前に置く | `int queueCapacity = default(64);` は受理、`int p = 1;`、`default(1+2)` は拒否 |
| 静的構造 | `gates:` は `input 名前;` または `output 名前;`。`submodules:` は `名前: 完全修飾型名;` のみ。子はsimple/moduleのみ。同じ親内の子名、同じ型内のgate名の重複を拒否する | `a: demo.Controller;` は受理、`a: Controller;`、`a[2]: demo.Controller;` は拒否 |
| 型参照 | 子型・channel型・設定networkは全てpackageを含む完全修飾名で照合。完全名には少なくとも一つのドットを要求する。注：既定packageは採用範囲外。同一完全名の二重定義を、使用有無にかかわらず拒否する。異なるpackageの同じ末尾名は別型。全ファイルの全宣言の参照・再帰包含を検査し、未選択networkに属する異常も拒否する | `a.Node` と `b.Node` は共存、未知 `c.Node` は拒否 |
| 展開 | networkキーの型がnetworkであることを確認。rootインスタンス名はnetwork型の末尾名。子の完全パスは `Root.child.grandchild`。型包含循環を全型で拒否し、宣言順の深さ優先で静的展開する。インスタンス化の対象は選択したnetworkとその子孫に限定する | network `demo.Main` はroot `Main`、`Main.a` と `Main.b` は別実体 |
| 対象外書式 | import、extends、like、interfaces、types節、条件・ループ、ベクトル、inout、双方向／逆向き矢印、allowunconnected、動的接続、NEDインスタンス別代入、式、関数呼出しは未対応入力として診断する | `a.tx <--> b.rx;`、`connections allowunconnected:` は拒否 |


| 字句・構文補足 | 確定規則 |
| --- | --- |
| 字句境界 | トークン間の空白はASCII SP、TAB、LF、CRLFだけ。コメントは文字列外でのみ認識し、一個の空白と同じ区切りとして扱う。`//` は改行またはEOFまで、`/*` は次の `*/` まで。識別子・数値・記号は最長一致で読み、識別子の途中のコメントは別トークンになる。`-->` は連続する三文字。注：単独CR、NBSP等の非ASCII空白、未終端コメントは字句エラー |
| 予約語集合 | `package simple module network channel parameters gates submodules connections int double bool string input output default true false import extends like interfaces types inout allowunconnected for if moduleinterface channelinterface volatile xml object` を完全一致・大小文字区別で予約する。識別子IDは前表のASCII字句からこの集合を除いたもの。属性名class/display/description/unitと単位名はIDとしても使用可 |
| 正規名 | `QName = ID ("." ID)+`、`InstancePath = ID ("." ID)*`、`Endpoint = ID ["." ID]`。NEDでは各トークン間の空白・コメントを除いて照合する。元の表記に空白があっても保存する完全名・パス・接続識別には空白を含めない。packageだけは一成分 `ID` も受理する。package各成分と親ディレクトリ名はUTF-8バイトで完全一致させる |
| 完全消費 | 以下の生成規則を使い、ファイル末尾まで一意に解析する。`{X}` は0回以上、`[X]` は0回または1回、`A / B` は選択、引用符内は終端文字、ID/QName/Literal/Stringは本書と設定仕様の字句（Literalは数値・単位付き数値・bool・Stringのいずれか、Stringは二重引用文字列）。Unitは `s`、`bps`、`B` のいずれか。注：これらのメタ記号自体は入力文字ではない |
| ファイル・宣言生成規則 | `File = "package" ID {"." ID} ";" Declaration {Declaration} EOF`。`Declaration = Kind ID "{" [Parameters] [Gates] [Submodules] [Connections] "}"`。Kindはsimple/module/network/channel。種別ごとの節制限は前表を適用。空節は受理する |
| parameters生成規則 | `Parameters = "parameters" ":" {Parameter / TypeProperty}`。`Parameter = ScalarType ID {ParameterProperty} ["=" "default" "(" Literal ")"] ";"`。ScalarTypeはint/double/bool/string。`TypeProperty = "@" ("class" / "display" / "description") "(" String ")" ";"`。`ParameterProperty = "@" "unit" "(" Unit ")" / "@" "display" "(" String ")" / "@" "description" "(" String ")"`。各 `@`・属性名・括弧は独立トークンとして空白を受理。属性順は任意で、属性重複・適用位置は第2節で検査する |
| 構造生成規則 | `Gates = "gates" ":" {("input" / "output") ID ";"}`。`Submodules = "submodules" ":" {ID ":" QName ";"}`。`Connections = "connections" ":" {Endpoint "-->" [QName "-->"] Endpoint ";"}`。最初の矢印後から次の矢印またはセミコロンまでを読み、矢印があればQName、セミコロンならEndpointと判定する |
| 名前空間 | 型はpackage内で一意、パラメータ・gate・子は各宣言種別内で一意とする。異なる宣言種別で同じIDを用いても、節と端点文脈から識別する。子型・channel型は後続ファイルや後続宣言も参照できる。注：同じ種別内の重複は後発宣言を原因位置として診断する |
| 全宣言と選択実体 | 使用有無にかかわらず字句・構文・型参照・種別・登録スキーマ・default型単位値域・包含循環・接続の静的整合を全宣言で検査する。値の必須欠落、INI上書き、合算delay、CAN構成検証は選択networkの展開実体に対して検査する。未選択型のdefaultなしパラメータは宣言として有効。root境界条件はnetwork宣言ごとに適用する。独立したmodule型の外側gateは親で接続されるため、その型単独の検査では外側未接続を欠陥としない |
| 静的接続検査 | 各compoundの直下型から端点の存在・方向・各内側接続数を照合し、子の各外側接続数を照合する。登録済みsimpleを端点として境界経路を追跡し、protocol/message/versionの一致と循環を検査する。この検査は型構造上で実施可能であり、未選択networkでも同じ判定になる |
| 元位置 | 全宣言・属性・接続・値の開始トークンと終端直後の位置を保持する。行列・BOM・改行の扱いと複数不正の検証順は[診断仕様](診断詳細機能仕様書.md#diagnostic-schema)に従う。構文エラーは最初の期待外トークン、未終端文字列／コメントは開始引用符／開始スラッシュ、EOFでの欠落はEOF位置を原因位置にする |

## 2. 実装登録とプロパティ

[実装対応](../機能仕様書.md#dir-func-0010)は型名をキーに登録情報を確定する。[共通モデル](../機能仕様書.md#dir-func-0006)はこの登録を通じて能力を合成する。

<a id="implementation"></a>

```trace
{
  "id": "spec-ned#implementation",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0019",
    "DIR-REQ-0028",
    "DIR-REQ-0029",
    "DIR-REQ-0051"
  ],
  "upstream": [
    "DIR-FUNC-0004",
    "DIR-FUNC-0006",
    "DIR-FUNC-0010"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定規則 |
| --- | --- |
| 登録の二段階 | コンパイル時Rust実装レジストリに実装キー・種別・パラメータスキーマ・ポートスキーマ・生成処理を登録する。公開登録契約はmodule/channelの種別ごとに `register_module(implementation_key, factory, descriptor)` / `register_channel(implementation_key, factory, descriptor)` とし、同じ実装キーの二重登録を拒否する。NEDのsimple/channelは `parameters: @class("実装キー");` を一つ必須とし、読込時にNED完全修飾型名→登録実装への対応表を作る。未知キー・種別違い・宣言スキーマ不一致は拒否。同一実装キーを別NED型で再利用可。注：動的ライブラリ、スクリプト、ネットワークからの実装追加は採用範囲外 |
| builtinとNED | `dir.can.Controller`、`dir.can.Bus`、`dir.link.FixedDelay` はRust実装キー。NED型は利用者が供給する明示宣言から登録する。標準配布のNED型にも同じ完全名を用いるが、例の `demo.Controller` 等も対応付け可能 |
| Controller schema | パラメータは `int queueCapacity`、`double txProcessingDelay @unit(s)`、`double rxProcessingDelay @unit(s)`、`string rxFilter`。ポートは `output tx` と `input rx` の二つだけ。値域・フィルタ意味は[CAN仕様](models/CANモデル詳細機能仕様書.md#controller)を正とする |
| Bus schema | `double bitrate @unit(bps)`、`string profile`。ポートは `input tx_SUFFIX` と `output rx_SUFFIX` の一対を接続ノードごとに明記。SUFFIXはASCII `[A-Za-z_][A-Za-z0-9_]*` に一致する非空文字列（全gate名はIDであり、suffixだけには予約語除外を適用しない）、両側で同一集合、最低二対。Rustの登録規則は名前パターンと対の制約を検証し、展開後は各scalarポートの情報を個別登録する。余分なパラメータ・ポートは拒否 |
| FixedDelay schema | channelのみ。パラメータは `double delay @unit(s)` の一つだけ。0以上の整数psで表現できる値。送信容量、帯域、競合、損失、確率を持たない |
| 属性の採用一覧 | `@class("...")` はsimple/channel型のparameters節、`@display("...")` と `@description("...")` は全型のparameters節、`@unit(s)`、`@unit(bps)`、`@unit(B)` は数値パラメータ宣言上のみ受理。unitは一つ、class/display/descriptionは型ごと各一つ。display/descriptionをパラメータ宣言に付すことも各一つ認める。その他の位置・未知属性・添字付き・複数引数・重複属性は拒否 |
| 保持形式 | 属性は所有者ID、属性名、文字列値、元ファイル・開始行列を保持。class/display/descriptionの文字列はエスケープ解釈後のUnicodeを保存し、unitは単位トークンを保存。注：displayの描画とdescriptionの実行は採用範囲外。各型・パラメータに紐付け、インスタンスから型のメタデータを参照可能にする |
| 登録照合 | パラメータは名前集合・宣言型・物理量（unitなしを含む）をdescriptorと照合する。ポートは展開後の名前集合・方向を照合し、protocol/message/versionはdescriptorから付与する。宣言順、default有無と値、display/descriptionは同一schema判定から独立させ、default値には登録された値域を適用する。compound/networkのパラメータは宣言をschemaとし、値を保持する。注：親子への暗黙転送・式参照は採用範囲外 |
| 実装キーと所有者 | class文字列はエスケープ解釈後に登録キーと完全一致で照合する。空文字は未登録キーとして診断する。型属性の所有者IDは `(type, NED完全修飾型名)`、パラメータ属性は `(parameter, NED完全修飾型名, パラメータ名)` の種別付き組。インスタンス自身に属性を複製せず型の所有者IDを保持する |
| 拡張境界 | 新実装はレジストリの別キーとスキーマ追加で行う。核の階層・接続処理へCAN固有の個数や型名を埋め込まない。初期版の単一バス制限はCAN profileの検証として適用する |

## 3. 結線・channel

[接続検証](../機能仕様書.md#dir-func-0005)を以下で定める。ポートは各simpleの登録情報を正とし、compoundの境界を経由しても端点の意味を保持する。

<a id="connections"></a>

```trace
{
  "id": "spec-ned#connections",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0020",
    "DIR-REQ-0021",
    "DIR-REQ-0022",
    "DIR-REQ-0023",
    "DIR-REQ-0073",
    "DIR-REQ-0074",
    "DIR-REQ-0075",
    "DIR-REQ-0076",
    "DIR-REQ-0097",
    "DIR-REQ-0005"
  ],
  "upstream": [
    "DIR-FUNC-0005",
    "DIR-FUNC-0021"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 項目 | 確定規則 | 正常／異常の期待結果 |
| --- | --- | --- |
| 接続文法 | `始点 --> 終点;` または `始点 --> 完全修飾channel型 --> 終点;`。端点は直下の `子名.gate名` または自身の境界 `gate名`。各文は一つの辺。注：channelの名前付け・インライン本体は採用範囲外 | `a.tx --> demo.Wire --> bus.tx_a;` は受理 |
| 方向 | 外から見て子のoutputが始点、inputが終点。自身のcompound inputは内側の始点、outputは内側の終点。任意祖先・孫への直接結線は拒否。自己境界同士の接続は方向が適合すれば受理する | 子input→子inputは方向エラー |
| 必須と重複 | simple scalarは接続相手ちょうど一つ。非root compound各gateは内側一つ・外側一つ。両側を独立の接続側として検証する。root networkのgate宣言は不正入力として診断する。二回目の同一接続側使用、未接続、同一辺再記述を拒否する | compound inputの外と内に一つずつは受理、内側二つは拒否 |
| 適合 | 任意の登録実装ではdescriptorのprotocol/messageキーの完全一致を適合条件とし、異なる型の接続は不適合として診断する。Controller.txとBus.tx_SUFFIXはprotocol `can.cc.ideal.v1`／message `CanTxRequest`（payload schema version 1）、Bus.rx_SUFFIXとController.rxは同protocol／message `CanNotification`（payload schema version 1）。protocolとmessageをピリオドで結合した正式schema名・schema版でEnvelopeを識別する。simpleの方向・名前はレジストリと完全一致必須。適合比較にはpayload schema versionも含める。境界の各有向経路をsimple始点・終点までたどりprotocol/messageの一致を検証する。型不一致、simpleへ達しない境界だけの循環、終端欠落は拒否 | — |
| channel経路 | channelなしはdelay=0。各辺のchannelは別インスタンスで、同じchannel型でもINI上書きは独立。境界経路に複数channelがあればdelayを整数psで加算し、u64桁あふれを拒否。各辺の識別は `親インスタンスパス::始点表記`（例 `Main::a.tx`、内側始点は `Main.box::in`）。この識別は方向側も一意に定める | — |
| channelの動作責任 | FixedDelayは経路遅延値をモデルへ提供する。CAN profileでは勝者の送信側経路delay＋受信者側経路delayをフレーム完了後の観測時刻へ一回だけ加算する。制御要求・仲裁開始は遅延加算の対象外とする（注：集中バスモデルの適用範囲）。フレーム占有時間はBusだけが一回計算する。核はモデルから指定された配送時刻をそのまま予約する | — |
| 共有バス | 各Controllerのtx/rxが同じBusの同じSUFFIX対へ到達することを検査。異なるController間の混在対や別Busへの片側接続は拒否。CAN初期profileは一つのBusと二つ以上のControllerだけを持つネットワークを受理（compoundによる包装可）。複数Bus対応はモデルprofileの拡張として扱う | — |

## 4. 最小の完全入力と境界例

[接続妥当性の親要件](../要件定義書.md#dir-req-0021)は、ノード存在・ポート存在・方向・protocol/message/schema版の各判定を組み合わせて確認する。compound境界の内側・外側はそれぞれの接続側を検査し、simpleの端点まで経路を解決して初めて配送可能な接続となる。構造上の適合に加えて、同じControllerのtx/rxが同じBusの対へ届くという[CAN構成条件](models/CANモデル詳細機能仕様書.md)を満たすことが、選択networkを初期化へ渡す共同の確認境界である。

<a id="examples"></a>

```trace
{
  "id": "spec-ned#examples",
  "stage": "spec",
  "requirements": [
    "DIR-REQ-0015",
    "DIR-REQ-0016",
    "DIR-REQ-0018",
    "DIR-REQ-0020",
    "DIR-REQ-0021",
    "DIR-REQ-0022"
  ],
  "upstream": [
    "DIR-FUNC-0002",
    "DIR-FUNC-0003",
    "DIR-FUNC-0005"
  ],
  "state": "confirmed",
  "pending": []
}
```

| 配置・入力 | 内容または期待結果 |
| --- | --- |
| `models/demo/Main.ned` 全文 | `package demo;`<br>`simple Controller { parameters: @class("dir.can.Controller"); int queueCapacity = default(64); double txProcessingDelay @unit(s) = default(0ps); double rxProcessingDelay @unit(s) = default(0ps); string rxFilter = default("*"); gates: output tx; input rx; }`<br>`simple Bus { parameters: @class("dir.can.Bus"); double bitrate @unit(bps); string profile = default("can.cc.ideal.v1"); gates: input tx_a; output rx_a; input tx_b; output rx_b; }`<br>`channel Wire { parameters: @class("dir.link.FixedDelay"); double delay @unit(s) = default(0ps); }`<br>`network Main { submodules: a: demo.Controller; b: demo.Controller; bus: demo.Bus; connections: a.tx --> demo.Wire --> bus.tx_a; bus.rx_a --> demo.Wire --> a.rx; b.tx --> demo.Wire --> bus.tx_b; bus.rx_b --> demo.Wire --> b.rx; }` |
| `scenario.ini` 全文 | `[General]`<br>`network = demo.Main`<br>`ned-path = "models"`<br>`sim-time-limit = 1ms`<br>`Main.bus.bitrate = 500kbps` |
| 起動・結果 | `dir-simulator run --config scenario.ini --output results`。root一つ、simple三つ、channel四つ、全経路delay=0、bitrate=500000bps、queueCapacity各64。workload省略のためフレーム生成0。入力受理・初期化可能な空負荷例でありCANフレーム検証の実行証跡ではない |
| compound受理例 | 上記Controllerと同じpackageに `module Box { gates: input in; output out; submodules: c: demo.Controller; connections: in --> c.rx; c.tx --> out; }` を宣言。Main.aの型をdemo.Boxに変えa.txをa.out、a.rxをa.inへ変えれば同一の端点に到達する |
| channel上書き | INIへ `[Channel Main::a.tx]`<br>`delay = 2ns` を追記。aの送信経路だけ2000ps、bの送信経路は0。a送信のb観測に2000psが追加される |
| 異常例 | a.txの辺を二度記述：重複接続側。b.rxへの辺削除：未接続。`a.unknown`：未知gate。`demo.Missing`：未知型。Mainにgate宣言追加：root境界禁止。いずれも `E-0001` の入力失敗、原因・ファイル行列・対象パスを通知し準備失敗として終了する |
| 独自型の再利用確認 | `channel OtherWire { parameters: @class("dir.link.FixedDelay"); double delay @unit(s) = default(1ns); }` を同じpackageに追加し二つの辺で `demo.OtherWire` を参照すると別インスタンス二つが各1000psを保持。`@class("unknown")` に変えると未登録実装として拒否 |
| 検証先 | [DIR-AC-0001](../要件定義書.md#dir-ac-0001)、[DIR-AC-0002](../要件定義書.md#dir-ac-0002)、[DIR-AC-0014](../要件定義書.md#dir-ac-0014)。仕様レビュー対象は収集・型・属性・結線の正常／異常対。[入力・設定詳細設計](../design/入力・設定詳細設計書.md)と[入力・設定検証仕様](../verification/cases/入力・設定検証仕様書.md)を規定済み。製品試験は未実施であり検証合格を表さない |

## 拡張モデルの型と構成

| 項目 | 規則・正本 |
| --- | --- |
| 構文 | 本書のscalarポート、compound境界、名前付きchannel、完全修飾名、@classを全profileで使用 |
| 実装登録 | GWはdir.can.MultibusController/MultibusBus、AXIはdir.axi.Manager/Interconnect/Ram、Ethernetは各モデル仕様のEndpoint/Switch/Linkを独立キーとして登録。各descriptorは固定のprotocol/message/schema版を持つ |
| 構成検証 | 既定CANのBus1個・Controller2個以上はcan.cc.ideal.v1の条件。追加profileは[GW](models/GWモデル詳細機能仕様書.md)、[AXI](models/AXIモデル詳細機能仕様書.md)、[Ethernet](models/Ethernetモデル詳細機能仕様書.md)の構成条件を適用 |
| coordinator | GWのcompound等に付く実行coordinatorはprofileのmodel-configから構築する。構造のcompoundは共通NED展開だけを担当し、追加の振る舞いはprofile側の登録契約に属する |
| channel | 接続端のpayload型適合と、profileが要求するchannel capabilityの適合を区別して確認。初期CANのFixedDelay条件を全モデルへ一律適用することは範囲注釈上の対象外 |

| 項目 | 契約 |
| --- | --- |
| Ethernet媒体型 | EndpointV2/SwitchV2/LinkV2を別の固定登録キーとして使用。scalarポート・channel・compound境界の既存構文で構成し、半二重の逆方向経路対はprofile側で一つの共有媒体へ結合する。[媒体仕様](models/Ethernet媒体拡張詳細機能仕様書.md) |

## 将来対応モデルへの適用

| 項目 | 契約 |
| --- | --- |
| 将来モデル | 追加モデルの固定登録型・scalar gate・配置条件は各正本を適用する。既存文法とRegistryで型を解決し、profile固有の構成を専用validatorで検証する。 |
| 正本 | [CAN FD・100BASE-T1](models/CANFD・100BASE-T1詳細機能仕様書.md)、[SoC・AHB・NoC](models/SoC・AHB・NoC詳細機能仕様書.md)、[DDR・SRAM・共有メモリIPC・DMA・メールボックスIPC](models/メモリ・IPC詳細機能仕様書.md) |
