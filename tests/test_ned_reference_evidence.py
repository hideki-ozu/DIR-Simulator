import copy,importlib.util,json,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('ned_evidence',ROOT/'scripts/check_ned_reference_evidence.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
class EvidenceTests(unittest.TestCase):
 def setUp(self):
  self.data=json.loads((ROOT/'docs/verification/results/ned-acceptance-residuals-2026-10-14.json').read_text());self.log=(ROOT/'docs/verification/results/ned-references-ci-final-2026-10-14.log').read_bytes()
 def test_actual_evidence(self):module.check(self.data,self.log)
 def test_wrong_filtered_run_is_rejected(self):
  for cid,name in module.NAMES.items():
   data=copy.deepcopy(self.data);case=next(x for x in data['cases'] if x['case_id']==cid)
   for c in case['conditions']:
    for e in c.get('evidence',[]):
     if e.get('named_test')==name:e['run']='rust_focused'
   with self.assertRaises((AssertionError,KeyError)):module.check(data,self.log)
 def test_missing_success_even_with_rehashed_log_is_rejected(self):
  import hashlib
  for name in module.NAMES.values():
   data=copy.deepcopy(self.data);log=self.log.replace(name.encode(),b'not_selected_test');data['runs']['ci_final_workspace_770c1e5']['log_sha256']=hashlib.sha256(log).hexdigest()
   with self.assertRaises(AssertionError):module.check(data,log)
 def test_modified_log_hash_is_rejected(self):
  with self.assertRaises(AssertionError):module.check(self.data,self.log+b'changed')
if __name__=='__main__':unittest.main()
