"""Bind approved CAN path conditions to saved named evidence, without new execution."""
from pathlib import Path
import argparse,hashlib,json,re
ROOT=Path(__file__).resolve().parents[2]
OUT=ROOT/'docs/verification/results/can-acceptance-paths-2026-10-10'
PR43='f86268956e876ed1f528a45ce6cb21219d6451a5'
MEASURED='9adf18faf1d0791360c4528fe1445b2560e395f8'
PACKET='docs/verification/results/ac0008-issue44-2026-10-10/'
def read(path): return json.loads((ROOT/path).read_text(encoding='utf-8'))
def testref(path,name):
    lines=(ROOT/path).read_text(encoding='utf-8').splitlines()
    line=next(i+1 for i,s in enumerate(lines) if re.match(r'fn '+re.escape(name)+r'\(',s))
    return dict(path=path,named_test=name,line=line,source_read_commit=PR43)
def build():
    runs=read(PACKET+'run-result.json');command=next(c for c in runs['commands'] if c['name']=='focused-tests')
    log=ROOT/PACKET/command['stdout'];digest=hashlib.sha256(log.read_bytes()).hexdigest()
    assert command['source_commit']==MEASURED and command['exit_code']==0
    assert digest==command['stdout_sha256']
    names={t['name']:t['outcome'] for t in command['named_tests']}
    pathfile='crates/dir-simulator/tests/ac0008_paths.rs';registry='crates/dir-simulator/tests/registry.rs'
    cases=[]
    rows=[
      ('CAN-BUILTIN-CLI','CLI run','run_config → default Registry → prepare_with_registry_and_source → BuiltinAdapter → runtime registered adapter branch → simulate_builtin → CAN Engine','classical_can_cli_run_matches_plain_observable_projection','CLI exit0 and exact simulation projection equals plain prepare/run; metadata excluded, arrays retain order'),
      ('CAN-BUILTIN-RUN-CONFIG','run_config','same default Registry/BuiltinAdapter/Engine path as CLI run','classical_can_run_config_matches_plain_observable_projection','RunReport exit0 and exact simulation projection equals plain prepare/run'),
      ('CAN-BUILTIN-PLAIN','prepare + run','input::prepare → prepare_with_source; Classical CAN registered=None; run → runtime builtin → CAN Engine','classical_can_prepare_and_run_preserve_analytic_competition','registered=None;2requests a:0,b:0;SOF0/106000000ps;EOF100000000/200000000ps;exit0;exact simulation projection'),
      ('CAN-BUILTIN-EXPLICIT','prepare_with_registry(default) + run','explicit default Registry → BuiltinAdapter; registered.is_generic=false → CAN Engine','classical_can_prepare_with_registry_and_run_uses_builtin_adapter','registered Some and is_generic=false;exit0;exact simulation projection equals plain'),
    ]
    for cid,api,path,name,expected in rows:
        assert names[name]=='ok' and 'test '+name+' ... ok' in log.read_text(encoding='utf-8')
        cases.append(dict(case_id=cid,acceptance='DIR-TEST-0085',api=api,implementation_path=path,expected=expected,test=testref(pathfile,name),execution=dict(status='passed_limited',source_commit=MEASURED,test_commit=MEASURED,published_at_commit=PR43,environment='WSL2 Ubuntu24.04.4 LTS;Rust/Cargo1.85.0;x86_64-unknown-linux-gnu',log=PACKET+command['stdout'],log_sha256=digest,run_result=PACKET+'run-result.json'),limits=['Observable equality is not factory/callback invocation evidence','competition fixture only; no universal model conformance'],full_ac0008_acceptance=False))
    for cid,api,path,expected,tests,limit in [
      ('CAN-VALIDATE-DIRECT','CLI validate','main validate → ordinary prepare → input::prepare_with_source; no run','Validate does not construct/execute CAN Engine or generic models',[],'Static source path only; no exact named validate counter test bound to this measured run'),
      ('CAN-EXPLICIT-RUN-CONFIG','run_config_with_registry','supplied Registry decides builtin adapter or generic Prepared','Default Registry builtin and custom Registry profile selections are distinct', ['registered_builtin_adapter_preserves_builtin_result','custom_run_publishes_registered_records_and_metrics'],'Existing builtin test uses gateway/fanout, not Classical CAN competition; no named registry integration execution log verified here'),
      ('GENERIC-PREPARE','prepare_with_registry(custom)','freeze/validate descriptors → model/channel configs; factories start in runtime','Successful prepare constructs no execution model; rejection starts no event callbacks',['custom_payload_channel_timer_cancel_and_arbitration_execute_deterministically'],'This concrete test covers runtime results, not factory counter0 during prepare'),
      ('GENERIC-FACTORY-LIFECYCLE','custom Registry + run/runtime::simulate','generic registered Engine initializes factories/models/channels; executes callbacks; finishes/releases','Factory selection/input/order, allocate failure, initialize failure, callback failure, reverse cleanup and frozen prefix must be observed separately',['acceptance_lifecycle_failures_release_models_and_keep_frozen_prefix','initialization_error_discards_effects_and_finishes_successful_initializations'],'Lifecycle CALLS includes allocate/init/finish/drop; no exact saved named registry execution log tied here'),
      ('GENERIC-CALLBACK-EFFECTS','custom Registry + runtime::simulate','generic registered Engine → custom on_event/on_arbitration/channel capability','Normal registered deliveries2/5ps, committed_events3, record byte7; callback failure leaves committed2/pending1/points1 and no model records',['custom_payload_channel_timer_cancel_and_arbitration_execute_deterministically','callback_failure_discards_every_effect_and_preserves_pending_current'],'Concrete assertions read; existing aggregate569/29 does not prove these named tests individually'),
    ]:
        cases.append(dict(case_id=cid,acceptance='DIR-TEST-0085',api=api,implementation_path=path,expected=expected,concrete_tests=[testref(registry,t) for t in tests],execution=dict(status='not_tested',source_commit=None,test_commit=None),limits=[limit],full_ac0008_acceptance=False))
    bindings=read(PACKET+'runtime-bindings.json')['bindings']
    probes=[b for b in bindings if b['observed']['callback_count_asserted']]
    assert len(probes)==4
    for i,b in enumerate(probes,1):
        assert names[b['named_test']]=='ok' and b['source_commit']==MEASURED
        assert b['observed']['callback_count']==0 and b['observed']['callback_positive_control_count']==1
        cases.append(dict(case_id=f'GENERIC-REJECTION-CALLBACK-{i:02d}',acceptance='DIR-TEST-0085',api='public prepare with custom registered positive control',expected='Normal input executes real callback once; after reset rejected input executes zero callbacks; exact rejection diagnostic preserved',test=dict(path='crates/dir-simulator/tests/ac0008_ned_rejections.rs',named_test=b['named_test']),execution=dict(status='passed_limited',source_commit=MEASURED,test_commit=MEASURED,log=PACKET+command['stdout'],log_sha256=digest,binding=PACKET+'runtime-bindings.json',binding_case_id=b['case_id'],positive_control=b['positive_control_evidence'],callback_count=0,positive_callback_count=1),limits=['Only3Issue44 rejection cases and unknown property','Does not prove Classical CAN factory callbacks or all generic lifecycle conditions'],full_ac0008_acceptance=False))
    return dict(schema_version=1,document_version='1.1.0',document_history=[dict(version='1.1.0',date='2026-10-10',change='Independent initial push after approved CAN acceptance decomposition')],approval=dict(decision_utc='2026-10-10T06:09Z',by='user',scope='Separate built-in CAN and generic extension acceptance; no CAN factory migration',not_approved=['all tests passed','AC0008 overall','license adoption','merge/release']),baseline=PR43,product_executed_in_this_task=False,existing_WSL_evidence_reused=True,source_navigation='source-navigation.json',cases=cases,historical_results_unchanged=True,remaining=['unbound named registry execution evidence','full atomic NED fixtures/assertions','origin/adoption/static-link population residuals'],full_ac0008_acceptance=False)
def main():
    p=argparse.ArgumentParser();p.add_argument('--check',action='store_true');a=p.parse_args()
    data=build();rendered=json.dumps(data,ensure_ascii=False,indent=2)+'\n';path=OUT/'acceptance-matrix.json'
    if a.check: assert path.read_text(encoding='utf-8')==rendered
    else: path.write_text(rendered,encoding='utf-8',newline='\n')
    print(f"{len(data['cases'])} conditions;8limited saved-evidence bindings;5not_tested;no new product run")
if __name__=='__main__': main()
