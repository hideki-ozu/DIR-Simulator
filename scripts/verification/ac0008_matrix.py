"""Expand DIR clauses/subcase alternatives without claiming product-test coverage.

The stable IDs name the current specification groups and individual clause atoms.
Full row context is retained so list items do not lose their qualifications.
Suite names are navigation candidates, never evidence that every atom passed.
"""
from pathlib import Path
import argparse, hashlib, json, re

FIXED='0b7b23d8e23fcb1a1491cd13cd42aa5b96a9ab1c'
SOURCE_TEST_COMMIT='6cd33fbf3b1cb09f61d628974559c560c519b47c'
ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'docs/verification/results/ac0008-review-2026-10-10'
SPEC='docs/specs/NED詳細機能仕様書.md'
CASES='docs/verification/cases/入力・設定検証仕様書.md'
NED='crates/dir-simulator/src/input/ned.rs'
INPUT='crates/dir-simulator/src/input.rs'
DEFAULT='crates/dir-simulator/src/input/tests.rs'

def write(name,obj):
    return json.dumps(obj,ensure_ascii=False,indent=2)+'\n'

def outside_split(text, separators):
    """Do not split syntax literals; keep the unsplit parent row separately."""
    result=[]; part=''; code=False
    for c in text:
        if c=='`': code=not code
        if not code and c in separators:
            if part.strip(): result.append(part.strip())
            part=''
        else: part+=c
    if part.strip(): result.append(part.strip())
    return result or [text]

def table_rows(path):
    heading=''; anchor=''; table=0; row=0; fence=False
    for line,text in enumerate((ROOT/path).read_text(encoding='utf-8').splitlines(),1):
        if text.startswith('```'): fence=not fence
        if fence: continue
        if text.startswith('## '): heading=text[3:]; table=0; row=0
        match=re.match(r'<a id="([^"]+)"',text)
        if match: anchor=match[1]
        if text.startswith('| ---'): table+=1; row=0; continue
        if not text.startswith('|') or table==0: continue
        cells=[x.strip() for x in outside_split(text.strip('|'),'|')]
        if len(cells)<2: continue
        # A new table's header immediately precedes its separator.
        if cells[1] in ('確定規則','規則・正本','契約','内容または期待結果','期待結果','独立に算出する期待値／診断'): continue
        row+=1
        yield dict(path=path,line=line,heading=heading,anchor=anchor,table=table,row=row,cells=cells)

def location(path,symbol):
    text=(ROOT/path).read_text(encoding='utf-8').splitlines()
    for line,value in enumerate(text,1):
        if re.search(r'\bfn '+re.escape(symbol)+r'\b',value):
            return dict(path=path,symbol=symbol,line=line,commit=FIXED)
    raise AssertionError((path,symbol))

def test(path,symbol,case_id,assertion_limit):
    ref=location(path,symbol)
    if 'ac0008_' in path: ref['commit']=SOURCE_TEST_COMMIT
    return {**ref,'case_id':case_id,'assertion_limit':assertion_limit,'execution_status':'not_run' if 'ac0008_' in path else 'not_bound_to_atomic_WSL_log'}

def mapping(text,anchor):
    source=NED; symbol='parse'; label='sec:ned-ref:syntax'; tests=[]
    classification='DIR_restriction_or_contract'
    if any(w in text for w in ('探索','ルート','ファイル','UTF-8','symlink','取得','読取','収集')):
        source=INPUT; symbol='collect_ned'; label='sec:ned-ref:directory-structure'
        tests=[(DEFAULT,'root_overlap_missing_files_and_symlinks_fail')]
    elif any(w in text for w in ('属性','display','description','@class','実装キー','登録','schema')):
        symbol='validate_schema'; label='sec:ned-ref:properties'
        tests=[('crates/dir-simulator/tests/registry_diagnostics.rs','generic_unknown_types_and_declaration_schema_retain_ned_sources')]
    elif any(w in text for w in ('INI','上書き','値域','default','必須欠落','bitrate','i64','u64','容量','量','単位','literal')):
        symbol='resolve_values'; label='sec:ned-ref:param-assignment-order'
        tests=[(DEFAULT,'defaults_overrides_empty_workload_and_limits'),('crates/dir-simulator/tests/registry_diagnostics.rs','generic_parameter_bounds_select_default_and_override_sources')]
    elif any(w in text for w in ('接続','gate','ポート','channel','payload','protocol','message','循環route')):
        symbol='validate_connections'; label='sec:ned-ref:connections'
        tests=[('crates/dir-simulator/src/input/ned/tests.rs','non_can_rules_reject_payload_mismatch_through_compound_boundary'),(DEFAULT,'compound_boundary_channels_are_independent_and_sum')]
    elif any(w in text for w in ('compound','展開','包含','子','実体','階層','循環')):
        symbol='containment'; label='sec:ned-lang:compound-modules'
        tests=[('crates/dir-simulator/src/input/ned/tests.rs','non_can_rules_validate_unused_declarations_and_defaults_before_overrides')]
    elif any(w in text for w in ('package','完全名','QName','FQName')):
        symbol='parse'; label='sec:ned-ref:package-declaration'
        tests=[('crates/dir-simulator/tests/diagnostics.rs','ned_unknown_child_reference_points_to_type_token')]
    elif any(w in text for w in ('字句','空白','コメント','予約語','文字列','識別子','BOM','CRLF','EOF','トークン')):
        symbol='lex'; label='sec:ned-ref:comments'
        tests=[('crates/dir-simulator/tests/diagnostics.rs','ned_eof_has_empty_range_and_correct_unicode_scalar_column')]
    if 'import' in text:
        label='sec:ned-ref:imports'; classification='upstream_supported_DIR_unsupported'
        tests.append(('crates/dir-simulator/tests/ac0008_ned_rejections.rs','import_is_explicitly_rejected_with_ned_source'))
    if 'INI' in text: classification='DIR_contract_not_upstream_NED_compatibility'
    if '保持形式' in text or '実装キーと所有者' in text:
        source=NED; symbol='property'; label='sec:ned-ref:properties'
    public_file='ch-ned-lang.tex' if label.startswith('sec:ned-lang:') else 'appendix-ned-ref.tex'
    return location(source,symbol), dict(tag='omnetpp-6.4.0',file='doc/src/manual/'+public_file,label=label,
        url='https://github.com/omnetpp/omnetpp/blob/omnetpp-6.4.0/doc/src/manual/'+public_file), classification, tests

def normative_matrix():
    rules=[]; groups=[]
    for row in table_rows(SPEC):
        if not row['heading']: continue
        if row['heading'].startswith(('1.','2.','3.')): groups.append(row)
        if row['cells'][0] in ('検証先','正本'): continue
        section=row['heading'].split('.')[0] if row['heading'][0].isdigit() else 'B'+hashlib.sha256(row['heading'].encode()).hexdigest()[:8]
        group=f"NED-S{section}-T{row['table']:02d}-R{row['row']:02d}"
        for cell in range(1,len(row['cells'])):
            atoms=outside_split(row['cells'][cell],'。、')
            for atom,clause in enumerate(atoms,1):
                text=row['cells'][0]+': '+clause
                source,public,kind,seeds=mapping(text,row['anchor'])
                mismatch=(row['cells'][0] in ('保持形式','実装キーと所有者') or 'display' in clause and any(x in clause for x in ('保持','保存','参照')))
                rules.append(dict(rule_id=f'{group}-C{cell:02d}-A{atom:03d}',group_id=group,
                    public_tag_section=public,public_grammar=dict(tag='omnetpp-6.4.0',blob='2d54f3b4559d83d899b0ef1cf7d3adda38ef9e12',section='appendix-ned-grammar.tex'),
                    dir_anchor_clause=dict(path=SPEC,anchor=row['anchor'],heading=row['heading'],line=row['line'],group=row['cells'][0],clause=clause,parent_row=row['cells']),
                    alignment=kind,observable_expected_behavior=clause,source=source,
                    concrete_tests=[test(p,s,f'{group}-C{cell:02d}-A{atom:03d}', 'candidate navigation only; does not assert every atom in this row') for p,s in seeds],
                    positive_negative_probe=dict(case_id=f'{group}-C{cell:02d}-A{atom:03d}',positive='Review parent-row accepted example if present',negative='Review parent-row rejected example if present',status='not_tested'),
                    execution_evidence_reference='execution-status.json: fixed 569/29 is aggregate only; no atomic pass inferred',
                    status='mismatch' if mismatch else 'not_tested',corrective_action='Issue #41: retain metadata and execute regression' if mismatch else 'Review candidate assertions; add missing atom-specific positive/negative tests and bind exact WSL logs',
                    reviewer=None,date='2026-10-10',technical_mapping_prepared_by='Codex; not approval or origin attestation'))
    assert len(groups)==37,len(groups)
    # Normative prose before/after tables is indexed as separate reviewer items.
    fence=False
    for line,text in enumerate((ROOT/SPEC).read_text(encoding='utf-8').splitlines(),1):
        if text.startswith('```'): fence=not fence
        if fence or text.startswith(('|','#','<','文書','対象','###','予定')) or not text.strip(): continue
        if not any(w in text for w in ('求める','要求','適用','保持','拒否','必須','確認','契約','規則')): continue
        for atom,clause in enumerate(outside_split(text,'。'),1):
            source,public,kind,seeds=mapping(clause,'syntax')
            rules.append(dict(rule_id=f'NED-P-L{line:04d}-A{atom:03d}',group_id='normative-prose',public_tag_section=public,
                dir_anchor_clause=dict(path=SPEC,anchor='syntax',line=line,clause=clause,parent_paragraph=text),alignment=kind,
                observable_expected_behavior=clause,source=source,concrete_tests=[],positive_negative_probe=None,
                execution_evidence_reference='execution-status.json',status='not_tested',corrective_action='Review scope/context and bind explicit assertions',reviewer=None,date='2026-10-10'))
    return dict(schema_version=1,fixed_product_commit=FIXED,source_test_commit=SOURCE_TEST_COMMIT,core_group_count=37,
        granularity='table cells split outside code at Japanese sentence/list boundaries; full parent context retained; coverage is not inferred',
        public_blobs={'ch-ned-lang.tex':'fefd18278a098a1bf77ed5ac467abe866877c808','appendix-ned-ref.tex':'890fe81eb25ae6f6362c3eb0a520fa801cbfb216','appendix-ned-grammar.tex':'2d54f3b4559d83d899b0ef1cf7d3adda38ef9e12'},rules=rules)

def subcases():
    cases=[]
    for row in table_rows(CASES):
        if row['anchor'] not in {f'dir-test-{n:04d}' for n in range(60,70)}: continue
        for alternative,operation in enumerate(outside_split(row['cells'][0],'／/'),1):
            case_id=f"DIR-TEST-{row['anchor'][-4:]}-R{row['row']:02d}-A{alternative:02d}"
            source,public,kind,seeds=mapping(operation+' '+row['cells'][1],row['anchor'])
            mismatch=('属性' in row['cells'][1] and any(w in row['cells'][1] for w in ('保持','参照','複写'))) or 'factory' in row['cells'][1] and 'Controller' in row['cells'][1]
            cases.append(dict(rule_id=case_id,case_id=case_id,public_tag_section=public,
                dir_anchor_clause=dict(path=CASES,anchor=row['anchor'],line=row['line'],parent_row=row['cells']),
                alignment=kind,operation=operation,observable_expected_behavior=row['cells'][1],source=source,
                concrete_tests=[test(p,s,case_id,'candidate; full diagnostic/owner/callback oracle not proven') for p,s in seeds],
                expected_rejection=any(w in row['cells'][1] for w in ('拒否','失敗','不正','E-0001','error','不適合')),
                execution_evidence_reference='execution-status.json',status='mismatch' if mismatch else 'not_tested',
                corrective_action='Issue #41 or built-in acceptance proposal; product regression pending' if mismatch else 'Execute row/alternative-specific oracle on exact WSL source; do not count 24 unnamed is_err mutations as exhaustive diagnostics',
                reviewer=None,date='2026-10-10'))
    return dict(schema_version=1,fixed_product_commit=FIXED,source_test_commit=SOURCE_TEST_COMMIT,cases=cases)

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--check',action='store_true');args=parser.parse_args()
    outputs={'ned-rule-matrix.json':normative_matrix(),'ned-subcase-matrix.json':subcases()}
    for name,data in outputs.items():
        rendered=write(name,data);path=OUT/name
        if args.check: assert path.read_text(encoding='utf-8')==rendered,name
        else: path.parent.mkdir(parents=True,exist_ok=True);path.write_text(rendered,encoding='utf-8',newline='\n')
    print(f"37 core groups; {len(outputs['ned-rule-matrix.json']['rules'])} clause atoms; {len(outputs['ned-subcase-matrix.json']['cases'])} subcase alternatives; no product pass inferred")

if __name__=='__main__': main()
