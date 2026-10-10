from pathlib import Path
import datetime,hashlib,json,os,re,subprocess,time
BASE=Path('/tmp/dir-v1.1.4-preparation-2026-10-08')
REPO=BASE/'worktree'
OUT=BASE/'rust-gates'
OUT.mkdir()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
paths=[REPO/'Cargo.toml',REPO/'Cargo.lock',REPO/'rust-toolchain.toml',REPO/'crates/dir-simulator/Cargo.toml',REPO/'crates/dir-simulator/build.rs']
paths+=sorted(p for p in (REPO/'crates/dir-simulator/src').rglob('*') if p.is_file())
paths+=sorted(p for p in (REPO/'crates/dir-simulator/tests').rglob('*') if p.is_file())
record={'started_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source_sha256':{str(p.relative_to(REPO)):sha(p) for p in paths},'build_documents_sha256':{str(p.relative_to(REPO)):sha(p) for p in (REPO/'docs/specs').rglob('*.md')},'commands':[]}
(OUT/'before.json').write_text(json.dumps(record,ensure_ascii=False,indent=2)+'\n')
env=os.environ.copy()
env['CARGO_TARGET_DIR']=str(BASE/'target')
cargo='/home/hideki/.cargo/bin/cargo'
commands=[[cargo,'fmt','--all','--','--check'],[cargo,'clippy','--locked','--offline','--workspace','--all-targets','--','-D','warnings'],[cargo,'test','--locked','--offline','--workspace'],[cargo,'build','--locked','--offline','--release','-p','dir-simulator']]
for i,cmd in enumerate(commands):
 log=OUT/f'{i}.log'
 start=time.monotonic()
 with log.open('wb') as out:r=subprocess.run(cmd,cwd=REPO,env=env,stdout=out,stderr=subprocess.STDOUT)
 item={'argv':cmd,'env':{'CARGO_TARGET_DIR':env['CARGO_TARGET_DIR']},'exit_code':r.returncode,'wall_seconds':time.monotonic()-start,'log':str(log),'log_sha256':sha(log)}
 record['commands'].append(item)
 print(json.dumps(item),flush=True)
 if r.returncode:
  print(log.read_text()[-7000:],flush=True)
  record['status']='failed'
  (OUT/'gate.json').write_text(json.dumps(record,ensure_ascii=False,indent=2)+'\n')
  raise SystemExit(r.returncode)
 assert all(sha(REPO/rel)==h for rel,h in record['source_sha256'].items())
 assert all(sha(REPO/rel)==h for rel,h in record['build_documents_sha256'].items())
 if i==2:
  summaries=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;',log.read_text())
  record['tests']={'passed':sum(int(s[0]) for s in summaries),'failed':sum(int(s[1]) for s in summaries),'ignored':sum(int(s[2]) for s in summaries),'suites':len(summaries)}
  print(json.dumps(record['tests']),flush=True)
binary=BASE/'target/release/dir-simulator'
record['binary']={'path':str(binary),'bytes':binary.stat().st_size,'sha256':sha(binary)}
record['status']='passed'
record['finished_at_utc']=datetime.datetime.now(datetime.timezone.utc).isoformat()
(OUT/'gate.json').write_text(json.dumps(record,ensure_ascii=False,indent=2)+'\n')
print(json.dumps({'status':'passed','binary':record['binary'],'tests':record['tests']}),flush=True)
