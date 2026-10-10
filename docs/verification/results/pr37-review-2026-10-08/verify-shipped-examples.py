from pathlib import Path
import hashlib,json,subprocess
BASE=Path('/tmp/dir-pr37-review-2026-10-08')
REPO=Path('/tmp/dir-v1.1.4-preparation-2026-10-08/worktree')
BIN=Path('/tmp/dir-v1.1.4-preparation-2026-10-08/target/release/dir-simulator')
OUT=BASE/'example-gates'
OUT.mkdir()
report={'binary_sha256':hashlib.sha256(BIN.read_bytes()).hexdigest(),'cases':[]}
for index,config in enumerate(sorted((REPO/'examples').rglob('*.ini'))):
 case={'config':str(config.relative_to(REPO)),'commands':[]}
 dest=OUT/f'{index:03d}'
 for op in ([str(BIN),'validate','--config',str(config)],[str(BIN),'run','--config',str(config),'--output',str(dest)],[str(BIN),'view','--input',str(dest/'results.json'),'--output',str(dest/'viewer.html')]):
  run=subprocess.run(op,cwd=REPO,capture_output=True,text=True)
  case['commands'].append({'argv':op,'exit_code':run.returncode,'stdout':run.stdout,'stderr':run.stderr})
  if run.returncode:
   print(json.dumps(case,ensure_ascii=False),flush=True)
   report['status']='failed'
   report['cases'].append(case)
   (OUT/'gate.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
   raise SystemExit(run.returncode)
 manifest=json.loads((dest/'manifest.json').read_text())
 entries=manifest['files']
 if isinstance(entries,dict):entries=[dict(v,path=k) for k,v in entries.items()]
 hashes=[]
 for f in entries:
  filename=f.get('path',f.get('name'))
  p=dest/filename
  actual=hashlib.sha256(p.read_bytes()).hexdigest()
  expected=f.get('sha256')
  size=f.get('bytes',f.get('size_bytes'))
  assert actual==expected and p.stat().st_size==int(size),(filename,f)
  hashes.append({'path':filename,'bytes':p.stat().st_size,'sha256':actual})
 case['manifest_files_verified']=hashes
 sim=json.loads((dest/'results.json').read_text())['simulation']
 case['termination']=sim['termination']
 case['partial']=sim['partial']
 report['cases'].append(case)
report['status']='passed'
report['total_cases']=len(report['cases'])
report['total_manifest_files']=sum(len(c['manifest_files_verified']) for c in report['cases'])
(OUT/'gate.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
print(json.dumps({'status':report['status'],'cases':report['total_cases'],'manifest_files':report['total_manifest_files']},ensure_ascii=False))
