"""Author-reviewed contract decomposition; static oracles, never execution passes."""
import argparse, hashlib, json, re
from pathlib import Path
import ac0008_matrix as m
ROOT=m.ROOT; OUT=m.OUT
VERSION='1.1.1'
# One independently observable predicate per item. Full original row remains attached.
CONTRACTS={
'ファイル':'UTF-8復号成功|先頭BOMを除外|LFとCRLFを同じ改行として扱う|小文字.ned通常fileを再帰収集|全指定rootは実在directory|root自身のsymlinkを拒否|配下directory symlinkを拒否|配下NED symlinkを拒否|配下非NED symlinkを拒否|重複rootを拒否|包含rootを拒否|root指定順を保持|root内相対pathをUTF-8 byte昇順で取得|隠しdirectoryも探索|非NED通常fileを無視|空root集合を拒否|読取不能入力を拒否|不正UTF-8を拒否',
'字句':'ASCII識別子を受理|識別子の大小文字を区別|予約語を識別子位置で拒否|空白をtoken区切りとする|行コメントを受理|非入れ子blockコメントを受理|文字列literal契約を適用|数値literal契約を適用|node_1を受理|node-1を拒否',
'package':'package宣言をfile先頭に要求|package各成分とroot-relative親directoryを一致照合|空packageを拒否|root直下NEDを拒否|package再宣言を拒否|file名とtype名の相違を受理',
'宣言':'package後の宣言数1以上|simple宣言を受理|module宣言を受理|network宣言を受理|channel宣言を受理|節順parameters/gates/submodules/connectionsを要求|同一節の重複を拒否|不要節省略を受理|simpleのsubmodules/connectionsを拒否|channelのgates/submodules/connectionsを拒否|module/networkの4節を受理|空本体はparse成功後schema検査する',
'パラメータ':'int宣言を受理|double宣言を受理|bool宣言を受理|string宣言を受理|default省略を必須値として保持|同一typeの同名parameterを拒否|parameter属性は値指定前に要求|default(literal)を受理|直接=1代入を拒否|default式1+2を拒否',
'静的構造':'input scalar gate宣言を受理|output scalar gate宣言を受理|QName子宣言を受理|子simpleを受理|子moduleを受理|子network/channelを拒否|親内の子名重複を拒否|type内gate名重複を拒否|非修飾子typeを拒否|子vectorを拒否',
'型参照':'子typeのQNameを照合|channel typeのQNameを照合|設定networkのQNameを照合|QNameに少なくとも1dotを要求|未使用でも重複QNameを拒否|異packageの同末尾名を別typeとして受理|全宣言の未知参照を拒否|未選択networkの異常参照を拒否|全宣言の包含循環を拒否',
'展開':'選択typeのkind=networkを要求|root名はnetwork末尾名|子pathはRoot.child.grandchild|全typeの包含循環を拒否|宣言順DFS展開|選択network子孫だけをinstance化|同型a/bを別instanceとして保持',
'対象外書式':'importを拒否|extendsを拒否|likeを拒否|interfacesを拒否|types節を拒否|if条件を拒否|forループを拒否|vectorを拒否|inoutを拒否|双方向矢印を拒否|逆向き矢印を拒否|allowunconnectedを拒否|動的接続を拒否|instance別代入を拒否|式を拒否|関数呼出しを拒否',
'字句境界':'SP区切りを受理|TAB区切りを受理|LF区切りを受理|CRLF区切りを受理|文字列内コメント記号を解釈しない|コメントを一個の空白相当とする|行コメントは改行又はEOFまで|blockコメントは次の終端まで|識別子を最長一致で読む|数値を最長一致で読む|記号を最長一致で読む|識別子途中コメントを別tokenにする|連続-->を要求|単独CRを拒否|NBSPを拒否|その他非ASCII空白を拒否|未終端コメントを拒否',
'予約語集合':'各予約語はID位置で拒否|予約語は完全一致で判定|予約語の大小文字を区別|予約語を含む長いIDは通常ID|classをIDとして受理|displayをIDとして受理|descriptionをIDとして受理|unitをIDとして受理|単位名をIDとして受理',
'正規名':'QNameはID(dot ID)を1回以上要求|InstancePathはID(dot ID)を0回以上受理|EndpointはID又はID.IDのみ|名前token間の空白を除外して照合|名前token間のコメントを除外して照合|保存QNameに空白を含めない|保存pathに空白を含めない|保存辺識別に空白を含めない|packageのみ1成分を受理|package成分とdirectoryをUTF-8 byte完全一致で照合',
'完全消費':'末尾までparseし余剰tokenを拒否|繰返し0回を受理|optional節0回又は1回|選択肢は指定されたliteral grammar|Stringは二重引用|unit sを受理|unit bpsを受理|unit Bを受理|メタ記号を入力終端として扱わない',
'ファイル・宣言生成規則':'File grammarはpackage+1以上Declaration+EOF|Declaration grammarはKind+ID+body|Kindはsimple/module/network/channel|種別ごとの節制限を適用|空parameters節を受理|空gates節を受理|空submodules節を受理|空connections節を受理',
'parameters生成規則':'parameters節はParameter又はTypePropertyの繰返し|ParameterはScalarType+ID+properties+optional default+semicolon|ScalarTypeはint/double/bool/string|TypeProperty classをStringとして受理|TypeProperty displayをStringとして受理|TypeProperty descriptionをStringとして受理|ParameterProperty unitをUnitとして受理|ParameterProperty displayをStringとして受理|ParameterProperty descriptionをStringとして受理|@と名前と括弧の間の空白を受理|属性順を任意として受理|属性重複を拒否|属性配置を第2節に照合',
'構造生成規則':'Gates grammarはinput/output ID semicolon|Submodules grammarはID colon QName semicolon|channelなしConnections grammarを受理|明示QName channelありConnections grammarを受理|矢印後の次矢印でQName channelと識別|矢印後のsemicolonで終点Endpointと識別',
'名前空間':'package内type名一意|宣言内parameter名一意|宣言内gate名一意|宣言内child名一意|異なる名前空間の同IDを受理|後続fileの子type参照を受理|後続宣言の子type参照を受理|後続fileのchannel参照を受理|後続宣言のchannel参照を受理|重複の後発宣言をprimary原因にする',
'全宣言と選択実体':'未使用宣言の字句を検査|未使用宣言の構文を検査|未使用宣言の参照を検査|未使用宣言のkindを検査|未使用宣言の登録schemaを検査|未使用defaultの型を検査|未使用defaultのunitを検査|未使用defaultの値域を検査|未使用宣言の包含循環を検査|未使用宣言の静的接続を検査|選択instanceだけに必須欠落を適用|選択instanceだけにINI上書きを適用|選択instanceの合算delayを検査|選択networkのCAN構成を検査|未選択typeのdefaultなしparameterを受理|各networkのroot境界禁止を検査|未instance化moduleの外側gate未接続を単独欠陥にしない',
'静的接続検査':'直下端点の存在を検査|直下端点の方向を検査|compound各内側接続数を検査|child各外側接続数を検査|登録simple端点まで境界routeを追跡|routeのprotocol一致を検査|routeのmessage一致を検査|routeのschema version一致を検査|境界route循環を拒否|未選択networkにも同じ静的判定を適用',
'元位置':'宣言start/endを保持|属性start/endを保持|接続start/endを保持|値start/endを保持|BOMを表示columnへ加算しない|CRLFを一改行に数える|複数不正のprimary順は診断正本に従う|構文不正は最初の期待外token位置|未終端stringは開始quote位置|未終端commentは開始slash位置|欠落EOFはEOF zero-width位置',
'登録の二段階':'module key/factory/descriptorを登録|channel key/factory/descriptorを登録|同一module key二重登録を拒否|同一channel key二重登録を拒否|simpleにclass1個必須|channelにclass1個必須|QNameからimplementation対応表を作成|未知keyを拒否|kind相違を拒否|schema不一致を拒否|別QNameによる同key再利用を受理|動的library/script/network登録を範囲外とする',
'builtinとNED':'dir.can.Controllerを実装keyとして扱う|dir.can.Busを実装keyとして扱う|dir.link.FixedDelayを実装keyとして扱う|NED明示宣言からtypeを登録|標準NEDの同名をtypeとkey別欄で保持|demo.Controllerを対応typeとして受理',
'Controller schema':'queueCapacity int/unitなし|txProcessingDelay double/unit s|rxProcessingDelay double/unit s|rxFilter string/unitなし|output txを要求|input rxを要求|余分なparameterを拒否|余分なportを拒否|CAN正本の値域を適用|CAN正本のfilter意味を適用',
'Bus schema':'bitrate double/unit bps|profile string/unitなし|scalar input名前任意ID|scalar output名前任意ID|input/output同数|各方向2個以上|gate名一意|gate名は予約語でない|役割を方向で決定|役割は接頭辞に依存しない|役割は接尾辞に依存しない|役割は宣言順に依存しない|同Controller入出力を一対登録|全portを一度だけ使用|余分なparameterを拒否|対応先なしportを拒否',
'FixedDelay schema':'kind channelのみ|delay double/unit sのみ|delay 0を受理|delay非負整数psを要求|delay u64範囲を要求|送信容量なし|帯域なし|競合なし|損失なし|確率なし',
'属性の採用一覧':'simple型classを受理|channel型classを受理|module/network型classを拒否|全type displayを受理|全type descriptionを受理|数値parameter unit sを受理|数値parameter unit bpsを受理|数値parameter unit Bを受理|bool/string parameter unitを拒否|各parameter unit1個まで|type class1個まで|type display1個まで|type description1個まで|parameter display1個まで|parameter description1個まで|未知属性を拒否|位置違反を拒否|添字付き属性を拒否|複数引数属性を拒否|重複属性を拒否',
'保持形式':'属性owner IDを保持|属性名を保持|属性Unicode文字列値を保持|属性元fileを保持|属性開始line/columnを保持|class escapeを復号|display escapeを復号|description escapeを復号|unit tokenを保持|display描画は範囲外|description実行は範囲外|type属性をtype ownerへ紐付け|parameter属性をparameter ownerへ紐付け|instanceからtype metadataを参照',
'登録照合':'parameter名前集合を照合|parameter宣言型を照合|parameter物理量を照合|unitなし物理量を照合|port名前集合を照合|port方向を照合|descriptorからprotocolを付与|descriptorからmessageを付与|descriptorからversionを付与|宣言順はschema同一性から除外|default有無はschema同一性から除外|default値はschema同一性から除外|displayはschema同一性から除外|descriptionはschema同一性から除外|default値域を登録boundに照合|compound/network宣言schemaを保持|compound/network値を保持|親子暗黙値転送を範囲外とする|式参照を範囲外とする',
'実装キーと所有者':'class復号文字列とkey完全一致|空class keyを拒否|type owner=(type,QName)|parameter owner=(parameter,QName,name)|instanceへ属性を複製しない|instanceはtype owner IDを保持',
'拡張境界':'別implementation keyで追加|descriptor schemaで追加|核の階層処理にCAN個数を埋め込まない|核の接続処理にCAN型名を埋め込まない|単一Bus制限をCAN profile validatorへ限定',
'接続文法':'channelなし有向辺を受理|QName channelあり有向辺を受理|直下child.gate端点を受理|自身境界gate端点を受理|1文を1辺として保持|channel名前付けを範囲外とする|inline channel bodyを範囲外とする',
'方向':'child outputは始点|child inputは終点|自身compound inputは内側始点|自身compound outputは内側終点|任意祖先への直接辺を拒否|孫への直接辺を拒否|適合方向の自己境界辺を受理|child input始点を拒否',
'必須と重複':'simple scalar接続数1|非root compound内側接続数1|非root compound外側接続数1|内側/外側を別接続側として計数|root network gateを拒否|2回目の同側使用を拒否|未接続を拒否|同一辺再記述を拒否',
'適合':'descriptor protocol完全一致|descriptor message完全一致|payload schema version完全一致|Controller.txとBus.inputはCanTxRequest/v1|Bus.outputとController.rxはCanNotification/v1|Envelopeは正式schema名と版を保持|simple宣言を登録schemaに照合|Bus任意gate名方向からdescriptorを作る|simple始点終点まで追跡|payload不一致を拒否|simpleに達しない境界循環を拒否|終端欠落を拒否',
'channel経路':'channelなしdelay=0|各辺channelは独立instance|同型channel INI上書きは独立|複数channel delayは整数ps加算|u64合算overflowを拒否|辺IDは親instance::始点|内部辺IDはMain.box::in|辺IDで方向側を一意識別',
'channelの動作責任':'FixedDelayは経路delayをモデルへ渡す|CAN観測は送信側+受信側delayを完了後に1回加算|制御要求へdelayを加算しない|仲裁開始へdelayを加算しない|Busだけがframe占有時間を1回計算|核はモデル配送時刻をそのまま予約',
'共有バス':'Controller.txからBus.inputへroute追跡|Bus.outputから同Controller.rxへroute追跡|同Bus入出力を一対として決定|Controller各一対|全Bus portがController1個に対応|送受信の別Busを拒否|対応欠落を拒否|対応重複を拒否|Bus間直接辺を拒否|同Bus内output宛先交換を受理|ideal profileはBus1個|ideal profileはController2個以上|compound包装を受理|複数Busはmultibus profileで扱う',
}
TESTFILES=['crates/dir-simulator/src/input/tests.rs','crates/dir-simulator/src/input/ned/tests.rs','crates/dir-simulator/tests/diagnostics.rs','crates/dir-simulator/tests/registry_diagnostics.rs','crates/dir-simulator/tests/registry.rs','crates/dir-simulator/tests/ac0008_ned_rejections.rs','crates/dir-simulator/tests/ac0008_paths.rs']
def catalogue():
    out=[]
    for path in TESTFILES:
        text=(ROOT/path).read_text(encoding='utf-8')
        starts=list(re.finditer(r'(?m)^fn (\w+)\(',text))
        for i,match in enumerate(starts):
            if not text[max(0,match.start()-30):match.start()].rstrip().endswith('#[test]'): continue
            end=starts[i+1].start() if i+1<len(starts) else len(text)
            body=text[match.start():end]; lines=body.splitlines()
            assertions=[]
            for n,line in enumerate(lines):
                if re.search(r'\b(assert(?:_eq|_ne)?!|assert_span\(|assert_range\()',line):
                    assertions.append(dict(line=text[:match.start()].count('\n')+n+1,excerpt='\n'.join(lines[n:n+min(8,len(lines)-n)])))
            out.append(dict(test_id=path+'::'+match[1],path=path,symbol=match[1],line=text[:match.start()].count('\n')+1,commit=m.SOURCE_TEST_COMMIT if 'ac0008_' in path else m.FIXED,assertions=assertions,assertion_status='direct_assertion_excerpt' if assertions else 'no_direct_assertion; helper invocation or no-panic loop only',test_body_excerpt=body if not assertions else None,execution_status='not_run' if 'ac0008_' in path else 'not_bound_to_atomic_WSL_log'))
    return out
# Reviewed navigation: limited to actual assertions, not whole-row coverage.
MAP={
'ファイル':['root_overlap_missing_files_and_symlinks_fail','unreadable_input_has_path_and_null_positions'],
'字句':['invalid_ned_declarations_and_wiring_are_rejected','ned_eof_has_empty_range_and_correct_unicode_scalar_column'],
'package':['invalid_ned_declarations_and_wiring_are_rejected'],
'宣言':['invalid_ned_declarations_and_wiring_are_rejected'],
'パラメータ':['invalid_ned_declarations_and_wiring_are_rejected','defaults_overrides_empty_workload_and_limits'],
'静的構造':['invalid_ned_declarations_and_wiring_are_rejected','vector_is_explicitly_rejected_with_ned_source','unqualified_child_type_is_explicitly_rejected_with_ned_source'],
'型参照':['generic_unknown_types_and_declaration_schema_retain_ned_sources','ned_unknown_child_reference_points_to_type_token','invalid_ned_declarations_and_wiring_are_rejected'],
'展開':['non_can_rules_resolve_values_compound_paths_and_shared_channels','compound_boundary_channels_are_independent_and_sum'],
'対象外書式':['import_is_explicitly_rejected_with_ned_source','inheritance_is_explicitly_rejected_with_ned_source','inout_is_explicitly_rejected_with_ned_source','allowunconnected_is_explicitly_rejected_with_ned_source'],
'字句境界':['ned_eof_has_empty_range_and_correct_unicode_scalar_column'],
'予約語集合':['invalid_ned_declarations_and_wiring_are_rejected'],
'正規名':['ned_unknown_child_reference_points_to_type_token'],
'完全消費':['truncated_ned_returns_diagnostics_without_panicking'],
'ファイル・宣言生成規則':['all_can_fixtures_load_and_preserve_snapshots'],
'parameters生成規則':['invalid_ned_declarations_and_wiring_are_rejected'],
'構造生成規則':['compound_boundary_channels_are_independent_and_sum'],
'名前空間':['invalid_ned_declarations_and_wiring_are_rejected'],
'全宣言と選択実体':['non_can_rules_validate_unused_declarations_and_defaults_before_overrides','unused_registered_declaration_defaults_still_obey_registry_bounds','unused_registered_double_and_channel_defaults_obey_bounds'],
'静的接続検査':['non_can_rules_reject_payload_mismatch_through_compound_boundary','invalid_ned_declarations_and_wiring_are_rejected'],
'元位置':['ned_eof_has_empty_range_and_correct_unicode_scalar_column','ned_unknown_child_reference_points_to_type_token','generic_parameter_bounds_select_default_and_override_sources'],
'登録の二段階':['duplicate_registration_does_not_replace_original','generic_unknown_types_and_declaration_schema_retain_ned_sources'],
'builtinとNED':['registered_builtin_adapter_preserves_builtin_result','classical_can_prepare_with_registry_and_run_uses_builtin_adapter'],
'Controller schema':['invalid_ned_declarations_and_wiring_are_rejected','all_can_fixtures_load_and_preserve_snapshots'],
'Bus schema':['bus_requires_equal_input_output_counts_and_two_controllers','bus_gate_names_and_same_bus_output_permutations_are_valid'],
'FixedDelay schema':['exact_units_and_extreme_decimal_precision','compound_boundary_channels_are_independent_and_sum'],
'属性の採用一覧':['invalid_ned_declarations_and_wiring_are_rejected'],
'保持形式':[],
'登録照合':['generic_unknown_types_and_declaration_schema_retain_ned_sources','generic_parameter_bounds_select_default_and_override_sources'],
'実装キーと所有者':[],
'拡張境界':['non_can_rules_resolve_values_compound_paths_and_shared_channels'],
'接続文法':['compound_boundary_channels_are_independent_and_sum'],
'方向':['invalid_ned_declarations_and_wiring_are_rejected'],
'必須と重複':['invalid_ned_declarations_and_wiring_are_rejected'],
'適合':['non_can_rules_reject_payload_mismatch_through_compound_boundary','unknown_schema_and_port_version_are_prepare_errors'],
'channel経路':['compound_boundary_channels_are_independent_and_sum','live_channel_settings_override_captured_initial_state_after_prepare'],
'channelの動作責任':['all_can_fixtures_load_and_preserve_snapshots','classical_can_prepare_and_run_preserve_analytic_competition'],
'共有バス':['bus_gate_names_and_same_bus_output_permutations_are_valid','bus_requires_equal_input_output_counts_and_two_controllers'],
}
LIMITS={
'invalid_ned_declarations_and_wiring_are_rejected':'24 unnamed mutation array + invalid/excessive default checks; asserts is_err only, not every reason/target/span/callback',
'truncated_ned_returns_diagnostics_without_panicking':'invokes parser on all UTF-8 prefixes; no acceptance/error/span assertion',
'root_overlap_missing_files_and_symlinks_fail':'overlap message, missing workload is_err, Unix non-NED symlink message only; no root symlink/order oracle',
'exact_units_and_extreme_decimal_precision':'explicit parse_time/quantity vectors only; not every INI key/int/double/schema case',
'all_can_fixtures_load_and_preserve_snapshots':'existing fixture has3Controllers+Bus and6channels; not0060 twoController+4edge fixture',
'classical_can_prepare_and_run_preserve_analytic_competition':'competition 2frames SOF/EOF; not0069 Box3000ps receiver oracle',
'unknown_schema_and_port_version_are_prepare_errors':'generic registered port/schema rejection; review fixture before claiming protocol/message/version independent probes',
}
NUMERIC_VARIANTS={
1:['sim-time-limit='+v for v in ('1ms','1000us','1000000ns')],
2:['txProcessingDelay='+v for v in ('0.001ns','0.1ps')],
3:['bitrate='+v for v in ('0.5Mbps','500kbps','0.5bps')],
4:[kind+' Main.bytes='+v for kind in ('int','double') for v in ('1KiB','1024B','1.024kB','0.5B')],
5:['sim-time-limit='+v for v in ('0ps','18446744073709551615ps','18446744073709551616ps')],
6:[key+'='+v for key in ('max-events','max-delta-cycles') for v in ('1','18446744073709551615','0','-1','1.0')],
7:['metrics-window='+v for v in ('0ps','1ps')],
8:['Main.signedValue='+v for v in ('-9223372036854775808','9223372036854775807','9223372036854775808','-0','64.0','0x40','+1','01')],
9:['Main.scalar='+v for v in ('0','0.0','-0','-0.0','0.5','1e3','NaN','Infinity')],
10:['Main.scalar='+('1'+'0'*309),'Main.scalar='+('0.'+'0'*399+'1')],
11:['sim-time-limit=1Mbps','txProcessingDelay=1','Main.signedValue=1ps'],
12:['Main.flag='+v for v in ('true','false','TRUE','1')]+['Main.text="a\\n\\t\\"\\\\"','Main.text="\\q"'],
13:['queueCapacity=0','queueCapacity=-1','bitrate=0bps','txProcessingDelay=-1ps'],
}
# Exact assertion correspondence where the fixture/assertion was read; absent entries
# are explicit per-case gaps, not filled using a keyword similarity score.
DIRECT={
'BOMを表示columnへ加算しない':('ned_eof_has_empty_range_and_correct_unicode_scalar_column','EOF range computes Unicode scalar column with BOM excluded; only this fixture'),
'欠落EOFはEOF zero-width位置':('ned_eof_has_empty_range_and_correct_unicode_scalar_column','exact EOF source/end range'),
'異なる名前空間の同IDを受理':None,
'input/output同数':('bus_requires_equal_input_output_counts_and_two_controllers','message equal counts for direction/count mutations'),
'各方向2個以上':('bus_requires_equal_input_output_counts_and_two_controllers','mutation removes b/c gates, checks schema count message'),
'同Bus内output宛先交換を受理':('bus_gate_names_and_same_bus_output_permutations_are_valid','swapped bus.tx_a/tx_b output routes: rx_channel 11000/7000ps'),
'役割は接頭辞に依存しない':('bus_gate_names_and_same_bus_output_permutations_are_valid','legacy/arbitrary names compare controller JSON and buses/channelcount'),
'役割は接尾辞に依存しない':('bus_gate_names_and_same_bus_output_permutations_are_valid','same explicit arbitrary-name variants; not all ID permutations'),
'u64合算overflowを拒否':('compound_boundary_channels_are_independent_and_sum','u64::MAXps plus1ps yields message overflow'),
'同型channel INI上書きは独立':('compound_boundary_channels_are_independent_and_sum','four edge overrides; tx5000/rx18000/other0/channelcount8'),
'重複rootを拒否':None,
'包含rootを拒否':('root_overlap_missing_files_and_symlinks_fail','models and models/demo; message overlapping'),
'配下非NED symlinkを拒否':('root_overlap_missing_files_and_symlinks_fail','cfg(unix) symlink.txt to Main.ned; message symlink'),
'読取不能入力を拒否':('unreadable_input_has_path_and_null_positions','missing-input acquisition only; not OS permission unreadability'),
'同一module key二重登録を拒否':('duplicate_registration_does_not_replace_original','duplicate registration preserves original; no channel equivalent inferred'),
'選択instanceだけにINI上書きを適用':('defaults_overrides_empty_workload_and_limits','a32/b64; max_events u64max; no generators; input count2'),
'未知keyを拒否':('invalid_ned_declarations_and_wiring_are_rejected','unknown @class mutation is_err; no exact reason/target/span'),
'子input始点を拒否':('invalid_ned_declarations_and_wiring_are_rejected','a.tx changed a.rx first edge, is_err only'),
'root network gateを拒否':('invalid_ned_declarations_and_wiring_are_rejected','network Main gains input in; is_err only'),
'2回目の同側使用を拒否':('invalid_ned_declarations_and_wiring_are_rejected','duplicate a.tx edge is_err; related-position assertion absent'),
'未使用defaultの値域を検査':('unused_registered_declaration_defaults_still_obey_registry_bounds','unused Sender count19 outside0..10; reason invalid_range target and exact NED span'),
'parameter宣言型を照合':('generic_unknown_types_and_declaration_schema_retain_ned_sources','int count to double count; invalid_type target demo.Sender.count and exact declaration span'),
'未使用宣言の未知参照を拒否':('invalid_ned_declarations_and_wiring_are_rejected','unselected Other child Missing is_err only'),
'importを拒否':('import_is_explicitly_rejected_with_ned_source','Fixture::reject helper: E-0001, nonempty reason, Main.ned source, line/column Some'),
'extendsを拒否':('inheritance_is_explicitly_rejected_with_ned_source','same reject helper; no fixed unsupported_syntax/callback0 oracle'),
'vectorを拒否':('vector_is_explicitly_rejected_with_ned_source','a[2] mutation and reject helper'),
'inoutを拒否':('inout_is_explicitly_rejected_with_ned_source','output tx -> inout tx and reject helper'),
'allowunconnectedを拒否':('allowunconnected_is_explicitly_rejected_with_ned_source','connections allowunconnected and reject helper'),
}
def exact_binding(predicate,cat):
    hit=DIRECT.get(predicate)
    if not hit: return []
    symbol,scope=hit;t=next(t for t in cat if t['symbol']==symbol)
    return [dict(test_id=t['test_id'],assertion_reference='test-assertion-catalogue.json#'+t['test_id'],static_binding='specific_input_and_assertion_read',observable_scope=scope,execution_status=t['execution_status'],full_diagnostic_contract_proven=False)]
def numeric_oracle(row,operation):
    value=operation.split('=',1)[1]
    if row==1: return '受理。正規時間1000000000ps。'
    if row==2: return '受理。正規時間1ps。' if value=='0.001ns' else '拒否。非整数ps換算1/10ps、精度不足を説明。reasonは診断正本との照合が必要。'
    if row==3: return '受理。正規速度500000bps。' if value!='0.5bps' else '拒否。非整数bps換算。'
    if row==4:
        if operation.startswith('int') and value in ('1.024kB','0.5B'): return '拒否。int係数の小数字句違反。物理量dataとdeclared_type intを混同しない。'
        return '拒否。doubleとして字句は有効だが0.5byteの非整数換算。' if value=='0.5B' else '受理。1024byte、物理量data、declared_typeはこのoperationのint又はdoubleを保持。'
    if row==5: return '拒否。u64最大+1、範囲超過。' if value=='18446744073709551616ps' else '受理。正規時間'+value[:-2]+'ps、binary64経由の丸めなし。'
    if row==6: return '受理。正規u64='+value+'、i64上限を適用しない。' if value in ('1','18446744073709551615') else '拒否。'+('invalid_range、値0。' if value=='0' else '実行keyの非負整数字句違反、負数/小数を採用しない。')
    if row==7: return '拒否。invalid_range、metrics-window0。' if value=='0ps' else '受理。metrics-window1ps。'
    if row==8: return '受理。正規i64='+('0' if value=='-0' else value)+'。' if value in ('-9223372036854775808','9223372036854775807','-0') else '拒否。'+('i64範囲超過。' if value=='9223372036854775808' else '採用int字句違反。')
    if row==9: return '受理。有限dimensionless double='+value+'。' if value in ('0','0.0','0.5') else '拒否。採用double字句違反（負符号付きゼロ/指数/非有限表記）。'
    if row==10: return '拒否。'+('binary64有限性overflow。' if value.startswith('1') else '非零十進値から0へのunderflow。')
    if row==11: return '拒否。'+{'sim-time-limit=1Mbps':'時間への速度unit不適合','txProcessingDelay=1':'必須時間unit欠落','Main.signedValue=1ps':'無unit宣言へのunit付加'}[operation]+'、targetと値全体span。単一reasonは診断正本照合まで未固定。'
    if row==12:
        if operation.startswith('Main.flag'): return '受理。bool='+value+'。' if value in ('true','false') else '拒否。小文字bool以外の字句。'
        return '拒否。不正escape q、開始値/原因位置を通知。' if '\\q' in value else '受理。復号値はa、改行、TAB、引用、backslashの順。二重escapeなし。'
    if row==13: return '受理。容量0。破棄動作は別CAN実行caseで検証。' if operation=='queueCapacity=0' else '拒否。'+{'queueCapacity=-1':'負容量のCAN値域','bitrate=0bps':'ゼロ速度のCAN値域','txProcessingDelay=-1ps':'負時間の共通時間条件'}[operation]+'。'
    raise AssertionError(row)
def references(group,cat):
    by={t['symbol']:t for t in cat};r=[]
    for symbol in MAP[group]:
        assert symbol in by,symbol
        t=by[symbol];r.append(dict(test_id=t['test_id'],assertion_reference='test-assertion-catalogue.json#'+t['test_id'],coverage='partial_navigation',limit=LIMITS.get(symbol,'Only literal fixture and assertions in catalogue; no execution or full predicate coverage inferred')))
    return r
def build():
    cat=catalogue();groups=[r for r in m.table_rows(m.SPEC) if r['heading'].startswith(('1.','2.','3.'))]
    assert len(groups)==37 and set(CONTRACTS)=={r['cells'][0] for r in groups}
    old=m.normative_matrix()['rules'];byp={}
    for r in old:
        c=r['dir_anchor_clause'];byp.setdefault(c.get('group'),r)
    atoms=[]
    for row in groups:
        group=row['cells'][0];gid=f"NED-S{row['heading'][0]}-T{row['table']:02d}-R{row['row']:02d}"
        for index,predicate in enumerate(CONTRACTS[group].split('|'),1):
            values=[None]
            if predicate=='各予約語はID位置で拒否':
                values=re.search(r'`([^`]+)`',row['cells'][1])[1].split()
            for variant,value in enumerate(values,1):
                aid=f'{gid}-O{index:03d}'+(f'-V{variant:02d}' if value else '')
                expectation=predicate+('：'+value if value else '')
                mismatch=group=='保持形式' and not any(s in predicate for s in ('描画','実行','class escape','unit token')) or group=='実装キーと所有者' and index>=3 or group=='元位置' and predicate.startswith('属性')
                previous=byp[group]
                atoms.append(dict(case_id=aid,group_id=gid,group=group,document_version=VERSION,dir_clause={'path':m.SPEC,'line':row['line'],'anchor':row['anchor'],'original_row':row['cells']},
                  predicate=expectation,oracle={'input_contract':row['cells'][2] if len(row['cells'])>2 else row['cells'][1],'operation':'基底/登録stage harnessに対象predicateだけの条件を作り、他の不正を除く。予約語variantはID位置へ単独置換。','expected_observable':expectation,'comparison':'Compare exactly this predicate; separate start/end owner/value/range assertions; do not substitute suite result','negative_diagnostic_contract':'E-0001 and diagnostic正本のreason/target/span/related/phase。正本が複数reasonを許す行は単独testでprimaryを記録し、推測固定しない'},
                  public_reference=previous['public_tag_section'],source_navigation=previous['source'],existing_concrete_tests=references(group,cat),status='mismatch' if mismatch else 'not_tested',execution_evidence=None,
                  exact_static_test_bindings=exact_binding(expectation,cat),missing_evidence=['WSL exact source/test log']+([] if exact_binding(expectation,cat) else ['No exact predicate-specific assertion correspondence established; see per-case gap index'])+(['Issue41 retained metadata implementation/accessor'] if mismatch else []),reviewer=None))
    # Original 0060..69 rows: no slash splitting inside quoted literals or commands.
    cases=[]
    for row in m.table_rows(m.CASES):
        if row['anchor'] not in {f'dir-test-{n:04d}' for n in range(60,70)}: continue
        operations=NUMERIC_VARIANTS[row['row']] if row['anchor']=='dir-test-0067' else m.outside_split(row['cells'][0],'／')
        # Each expected sentence is a complete context-qualified oracle; keep full parent row.
        for variant,operation in enumerate(operations,1):
            expectations=[numeric_oracle(row['row'],operation)] if row['anchor']=='dir-test-0067' else m.outside_split(row['cells'][1],'。')
            for expectation,expected in enumerate(expectations,1):
                cid=f"DIR-TEST-{row['anchor'][-4:]}-R{row['row']:02d}-V{variant:02d}-O{expectation:02d}"
                family=int(row['anchor'][-4:])
                candidates={60:['all_can_fixtures_load_and_preserve_snapshots','compound_boundary_channels_are_independent_and_sum'],61:['invalid_ned_declarations_and_wiring_are_rejected','ned_eof_has_empty_range_and_correct_unicode_scalar_column'],62:['duplicate_registration_does_not_replace_original','generic_unknown_types_and_declaration_schema_retain_ned_sources'],63:['compound_boundary_channels_are_independent_and_sum','non_can_rules_resolve_values_compound_paths_and_shared_channels'],64:['invalid_ned_declarations_and_wiring_are_rejected','non_can_rules_reject_payload_mismatch_through_compound_boundary'],65:['ini_rejects_unknown_duplicate_and_malformed_inputs','root_overlap_missing_files_and_symlinks_fail','structural_config_reference_errors_use_known_value_spans'],66:['defaults_overrides_empty_workload_and_limits','generic_parameter_bounds_select_default_and_override_sources','ini_unknown_parameter_points_to_complete_assignment_key'],67:['exact_units_and_extreme_decimal_precision','generic_execution_settings_use_adopted_ini_token_and_stable_reason'],68:['acceptance_lifecycle_failures_release_models_and_keep_frozen_prefix','initialization_error_discards_effects_and_finishes_successful_initializations'],69:['classical_can_prepare_and_run_preserve_analytic_competition','all_can_fixtures_load_and_preserve_snapshots']}[family]
                refs=[dict(test_id=t['test_id'],assertion_reference='test-assertion-catalogue.json#'+t['test_id'],coverage='partial_navigation',limit=LIMITS.get(t['symbol'],'Existing fixture/explicit assertions only; does not prove this oracle')) for t in cat if t['symbol'] in candidates]
                mismatch=family==61 and row['row'] in (1,2,3) and any(s in expected for s in ('属性','保持','参照')) or family in (62,68) and any(s in expected for s in ('Controller factory','Bus factory','ID0 a','ID1 b','ID2 bus'))
                cases.append(dict(case_id=cid,document_version=VERSION,dir_clause={'path':m.CASES,'line':row['line'],'anchor':row['anchor'],'original_row':row['cells']},operation=operation,expected_observable=expected,scope_qualifier='All qualifiers and later alternative subcases in original_row remain binding; sentence atom is not a new approved acceptance spec',existing_concrete_tests=refs,status='mismatch' if mismatch else 'not_tested',execution_evidence=None,reviewer=None,missing_evidence=['Exact subcase fixture / per-assertion binding / WSL log']))
    unresolved=[dict(key='metadata-owner-span',case_refs=['NED-S2-T01-R07','NED-S2-T01-R09','DIR-TEST-0061-R01','DIR-TEST-0061-R02','DIR-TEST-0061-R03'],reason='Declaration/Parameter storage/accessors absent. Values/owner/shared-type/span oracle defined, implementation absent; Issue41',decision='mismatch'),
      dict(key='can-factory-literal',case_refs=['DIR-TEST-0062-R01','DIR-TEST-0062-R02','DIR-TEST-0068-R01'],reason='Classical CAN uses BuiltinAdapter/legacy Engine; literal factories/recording hooks not demonstrated. Generic harness must be separated; wording approval pending',decision='mismatch'),
      dict(key='two-controller-fixture',case_refs=['DIR-TEST-0060-R01','DIR-TEST-0063','DIR-TEST-0069'],reason='Existing competition fixture has3Controllers and6channels, while specified baseline has2 and4. Cannot substitute counts or hook outputs',decision='not_tested'),
      dict(key='diagnostic-stage-order',case_refs=['DIR-TEST-0064-R09','DIR-TEST-0068-R02','DIR-TEST-0068-R03','DIR-TEST-0068-R04'],reason='Mixed invalid inputs need primary/stop-order stage harness. Existing unnamed is_err mutations do not bind exact reason/related/callback0',decision='not_tested'),
      dict(key='filesystem-hooks',case_refs=['DIR-TEST-0065-R09','DIR-TEST-0065-R10','DIR-TEST-0065-R12'],reason='Acquisition-after-enumeration deletion hook, normal-user unreadability and root/config/non-NED symlinks need environment-specific harness and operation/OS evidence',decision='not_tested'),
      dict(key='snapshot-after-prepare',case_refs=['DIR-TEST-0069-R04'],reason='Channel live settings test exists but no exact source-file replacement-after-prepare hash/adoptedbytes oracle found',decision='not_tested'),
      dict(key='public-section-mapping',case_refs=['public_tag_section in legacy matrices'],reason='Public labels are navigation candidates; not every clause has one-to-one upstream equivalent. DIR-specific grammar/profile rules remain DIR contracts',decision='not_tested'),
      dict(key='diagnostic-alternative-reason',case_refs=['DIR-TEST-0067-R02','DIR-TEST-0067-R11'],reason='Spec says invalid_range等/invalid_unit等. Do not invent unique reason; freeze exact oracle only with diagnosis正本 and isolated-source/test binding',decision='not_tested')]
    source_rows={}
    for a in atoms:
        sid=a['group_id'];source_rows[sid]=a['dir_clause'].pop('original_row');a['dir_clause']['source_row_id']=sid
        a['oracle']['input_contract']={'source_row_id':sid,'selection':'accepted/rejected example and qualifiers from complete source row'}
    for c in cases:
        sid=c['case_id'].split('-V')[0];source_rows[sid]=c['dir_clause'].pop('original_row');c['dir_clause']['source_row_id']=sid
    gaps=[dict(case_id=a['case_id'],predicate=a['predicate'],required='Isolated accepted/rejected fixture and exact predicate/diagnostic assertion; current group navigation is not that binding') for a in atoms if not a['exact_static_test_bindings']]
    return {'atomic-oracles.json':dict(schema_version=1,document_version=VERSION,fixed_product_commit=m.FIXED,new_test_commit=m.SOURCE_TEST_COMMIT,core_group_count=37,source_rows=source_rows,core_atoms=atoms,subcase_oracles=cases,execution_passes=0,coverage_claim='37 core groups and all source rows0060–0069 indexed; concrete missing bindings listed; not exhaustive executed conformance'),
      'predicate-binding-gaps.json':dict(document_version=VERSION,core_gaps=gaps,subcase_gaps=[dict(case_id=c['case_id'],operation=c['operation'],expected=c['expected_observable'],required='Exact stage/baseline fixture and complete diagnostics/owner/value/callback assertion binding not established') for c in cases]),
      'test-assertion-catalogue.json':dict(document_version=VERSION,tests=cat,assertion_excerpts_are_navigation=True),
      'unresolved-oracle-decisions.json':dict(document_version=VERSION,items=unresolved,all_conformance_claim=False)}
def main():
    p=argparse.ArgumentParser();p.add_argument('--check',action='store_true');a=p.parse_args()
    for name,data in build().items():
        rendered=json.dumps(data,ensure_ascii=False,indent=2)+'\n';path=OUT/name
        if a.check: assert path.read_text(encoding='utf-8')==rendered,name
        else: path.write_text(rendered,encoding='utf-8',newline='\n')
    d=build()['atomic-oracles.json']; print(f"37 groups / {len(d['core_atoms'])} predicates / {len(d['subcase_oracles'])} context-qualified subcase oracles; 0 execution passes")
if __name__=='__main__': main()
