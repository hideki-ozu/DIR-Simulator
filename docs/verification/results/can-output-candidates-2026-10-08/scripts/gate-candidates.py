import argparse,datetime,hashlib,json,re,shutil,subprocess,time
from pathlib import Path
ROOT=Path('/tmp/dir-opt-five-2026-10-08'); CARGO='/home/hideki/.cargo/bin/cargo'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def save(p,r):p.parent.mkdir(parents=True,exist_ok=True);p.write_text(json.dumps(r,indent=2)+'\n')
p=argparse.ArgumentParser();p.add_argument('--only',type=int,nargs='*',default=list(range(1,6)));a=p.parse_args()
for number in a.only:
 name='baseline' if number==0 else f'candidate{number}';work=ROOT/('baseline-source' if number==0 else name)
 stamp=datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S')
 evidence=ROOT/'gates'/name/stamp;evidence.mkdir(parents=True,exist_ok=False)
 report={'candidate':name,'source_directory':str(work),'commands':[],'started_at_utc':stamp}
 commands=[['fmt','--all','--','--check'],['clippy','--locked','--offline','--workspace','--all-targets','--target-dir',str(ROOT/'cargo-target'),'--','-D','warnings'],['test','--locked','--offline','--workspace','--target-dir',str(ROOT/'cargo-target')],['build','--locked','--offline','--release','--workspace','--target-dir',str(ROOT/'cargo-target')]]
 for i,args in enumerate(commands):
  log=evidence/f'{i}.log';start=time.monotonic()
  with log.open('w') as f:code=subprocess.run([CARGO,*args],cwd=work,stdout=f,stderr=subprocess.STDOUT).returncode
  report['commands'].append({'argv':[CARGO,*args],'cwd':str(work),'exit_code':code,'wall_seconds':time.monotonic()-start,'log':str(log),'log_sha256':sha(log)})
  save(evidence/'report.json',report)
  print(json.dumps({'candidate':name,'gate':args[0],'exit_code':code,'log':str(log)}),flush=True)
  if code:
   print(log.read_text()[-18000:],flush=True);raise SystemExit(code)
 sources={str(q.relative_to(work)):sha(q) for q in [*work.joinpath('crates').rglob('*'),work/'Cargo.toml',work/'Cargo.lock',work/'rust-toolchain.toml'] if q.is_file() and not q.is_symlink()}
 report['source_sha256']=sources
 destination=ROOT/'bin'/name;shutil.copy2(ROOT/'cargo-target/release/dir-simulator',destination)
 report['binary']={'path':str(destination),'sha256':sha(destination),'bytes':destination.stat().st_size}
 counts=re.findall(r'test result:.*?(\d+) passed; (\d+) failed; (\d+) ignored;', (evidence/'2.log').read_text())
 report['tests']={'passed':sum(int(x[0]) for x in counts),'failed':sum(int(x[1]) for x in counts),'ignored':sum(int(x[2]) for x in counts),'suites':len(counts)}
 save(evidence/'report.json',report);save(ROOT/'gates'/name/'latest.json',report)
 print(json.dumps({'candidate':name,'built':report['binary'],'tests':report['tests']}),flush=True)
