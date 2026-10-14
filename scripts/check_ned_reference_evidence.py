"""Check that NE-XC17/19 reference tests point to their recorded CI execution."""
from pathlib import Path
import hashlib,json,re
ROOT=Path(__file__).resolve().parents[1]
NAMES={'NE-XC17':'references_rejections_match_prepare_without_additional_acquisition','NE-XC19':'references_bom_crlf_escaped_json_and_channel_spans_keep_nonreferences_untouched'}
def check(data,log):
 for caseid,name in NAMES.items():
  case=next(x for x in data['cases'] if x['case_id']==caseid);assert case['status']=='partial'
  entries=[e for c in case['conditions'] for e in c.get('evidence',[]) if e.get('named_test')==name];assert len(entries)==1
  evidence=entries[0];run=data['runs'][evidence['run']];assert run['result']=='passed' and run['job_conclusion']=='success'
  assert run['command']=='cargo test --locked --workspace' and run['source_commit']=='770c1e582c83020908fd51fc58d41a6dee45898b' and run['job_id']==115584844166 and run['run_id']==38481473942
  assert run['log_sha256']==hashlib.sha256(log).hexdigest()
  assert re.search(r'test input::references::tests::'+name+r' \.\.\. ok$',log.decode(),re.M)
  assert run['lib_suite']=={'passed':442,'failed':0,'ignored':7,'filtered_out':0}
  assert any(c['status']=='unverified' for c in case['conditions'])
 print('PASS: two named tests map to exact final CI source, command, success lines/hash; partial/unverified retained')
def main():
 data=json.loads((ROOT/'docs/verification/results/ned-acceptance-residuals-2026-10-14.json').read_text());check(data,(ROOT/'docs/verification/results/ned-references-ci-final-2026-10-14.log').read_bytes())
if __name__=='__main__':main()
