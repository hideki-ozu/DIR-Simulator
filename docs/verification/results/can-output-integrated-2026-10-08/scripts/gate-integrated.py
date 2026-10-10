import json,hashlib,datetime,shutil,subprocess,time,re
from pathlib import Path
ROOT=Path('/tmp/dir-opt-integrated-2026-10-08');REPO=Path('/home/hideki/DIR-Simulator');CARGO='/home/hideki/.cargo/bin/cargo'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def save(p,r):p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(r,ensure_ascii=False,indent=2)+'\n')
stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S');evidence=ROOT/'gates'/stamp;evidence.mkdir(parents=True,exist_ok=False)
r={'source_directory':str(REPO),'commands':[],'started_at_utc':stamp}
commands=[['fmt','--all','--','--check'],['clippy','--locked','--offline','--workspace','--all-targets','--target-dir','/tmp/dir-opt-five-2026-10-08/cargo-target','--','-D','warnings'],['test','--locked','--offline','--workspace','--target-dir','/tmp/dir-opt-five-2026-10-08/cargo-target'],['build','--locked','--offline','--release','--workspace','--target-dir','/tmp/dir-opt-five-2026-10-08/cargo-target']]
for i,args in enumerate(commands):
 log=evidence/f'{i}.log';t=time.monotonic()
 with log.open('w') as f:code=subprocess.run([CARGO,*args],cwd=REPO,stdout=f,stderr=subprocess.STDOUT).returncode
 r['commands'].append({'argv':[CARGO,*args],'cwd':str(REPO),'exit_code':code,'wall_seconds':time.monotonic()-t,'log':str(log),'log_sha256':sha(log)})
 save(evidence/'report.json',r);print(json.dumps({'gate':args[0],'exit_code':code,'log':str(log)}),flush=True)
 if code:print(log.read_text()[-14000:],flush=True);raise SystemExit(code)
files=[*REPO.joinpath('crates').rglob('*'),REPO/'Cargo.toml',REPO/'Cargo.lock',REPO/'rust-toolchain.toml'];r['source_sha256']={str(p.relative_to(REPO)):sha(p) for p in files if p.is_file() and not p.is_symlink()}
r['build_documents_sha256']={str(p.relative_to(REPO)):sha(p) for p in REPO.joinpath('docs/specs').rglob('*') if p.is_file()}
b=ROOT/'bin/integrated';shutil.copy2(Path('/tmp/dir-opt-five-2026-10-08/cargo-target/release/dir-simulator'),b);r['binary']={'path':str(b),'sha256':sha(b),'bytes':b.stat().st_size}
counts=re.findall(r'test result:.*?(\d+) passed; (\d+) failed; (\d+) ignored;', (evidence/'2.log').read_text());r['tests']={'passed':sum(int(x[0]) for x in counts),'failed':sum(int(x[1]) for x in counts),'ignored':sum(int(x[2]) for x in counts),'suites':len(counts)}
save(evidence/'report.json',r);save(ROOT/'gates/latest.json',r);print(json.dumps({'binary':r['binary'],'tests':r['tests']}),flush=True)
