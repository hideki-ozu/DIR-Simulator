from pathlib import Path
import hashlib,json,shutil
ROOT=Path('/tmp/dir-million-integrated-2026-10-08')
REPO=Path('/home/hideki/DIR-Simulator')
REPORT=REPO/'docs/verification/results/can-million-integrated-2026-10-08.json'
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
 report=json.loads(REPORT.read_text())
 support=REPORT.parent/report['inventory_base_directory']
 gate=json.loads((ROOT/'documentation-gates/gate.json').read_text())
 assert gate['status']=='passed'
 assert all(c['exit_code']==0 and sha(Path(c['log']))==c['log_sha256'] for c in gate['commands'])
 inventory=report['support_files']
 def add(source,rel,kind):
  assert source.is_file() and not source.is_symlink()
  target=support/rel
  assert not target.exists()
  target.parent.mkdir(parents=True,exist_ok=True)
  shutil.copyfile(source,target)
  assert sha(source)==sha(target)
  row={'kind':kind,'original_local_path':str(source),'support_relative_path':rel,'bytes':target.stat().st_size,'sha256':sha(target)}
  inventory.append(row)
  return row
 for source in sorted((ROOT/'documentation-gates').iterdir()):
  add(source,'documentation-gates/'+source.name,'documentation_gate')
 current={}
 for index,rel in enumerate(('docs/品質・配布方針.md','docs/verification/cases/利用フロー・品質検証仕様書.md')):
  current[rel]=sha(REPO/rel)
  add(REPO/rel,'docs-after/'+rel+'.txt','updated_current_document')
  add(ROOT/'document-review'/f'{index}.patch',f'document-review/{index}.patch','reviewed_document_patch')
 add(Path(__file__),'append-documentation-evidence.py','documentation_evidence_controller')
 report['documentation_gate']={'status':'passed','support_relative_path':'documentation-gates/gate.json','sha256':sha(ROOT/'documentation-gates/gate.json')}
 report['updated_current_documents_sha256']=current
 report['support_files']=sorted(inventory,key=lambda x:x['support_relative_path'])
 REPORT.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
 expected={x['support_relative_path']:x for x in report['support_files']}
 assert len(expected)==len(inventory)
 actual={str(p.relative_to(support)):p for p in support.rglob('*') if p.is_file()}
 assert set(actual)==set(expected)
 for rel,p in actual.items():
  row=expected[rel]
  assert not p.is_symlink() and p.stat().st_size==row['bytes'] and sha(p)==row['sha256'],rel
 for rel,h in report['source_sha256'].items(): assert sha(REPO/rel)==h,rel
 for rel,h in report['build_documents_sha256'].items(): assert sha(REPO/rel)==h,rel
 assert sha(Path(report['binary']['path']))==report['binary']['sha256']
 assert sha(Path(report['prior_integrated_report']['path']))==report['prior_integrated_report']['sha256']
 print(json.dumps({'status':'passed','support_files':len(actual),'current_source_files':len(report['source_sha256']),'build_documents':len(report['build_documents_sha256']),'documentation_gate':'passed','prior_report_unchanged':True}))
if __name__=='__main__':main()
