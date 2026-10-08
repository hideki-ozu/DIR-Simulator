"""Capture immutable CLI evidence or compare native/WSL runs without sorting observations."""
import argparse, csv, hashlib, io, json, platform, subprocess, sys, tempfile, time
from decimal import Decimal
from pathlib import Path
TARGET = 'd7cb3861f9ac56646ac2dbacf6f324b12f7fcb1b'
CASES = ('competition', 'release-arrival')
ABS, REL = Decimal('1e-12'), Decimal('1e-9')
def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for b in iter(lambda: f.read(1024*1024), b''): h.update(b)
    return h.hexdigest()
def load(path): return json.loads(path.read_text(encoding='utf-8'), parse_float=Decimal)
def write(path, value): path.write_text(json.dumps(value, ensure_ascii=True, indent=2)+'\n', encoding='utf-8')
def equal(a,b,tolerant=False):
    if type(a) is not type(b): return False
    if isinstance(a,Decimal):
        return a==b or (tolerant and (abs(a-b)<=ABS or abs(a-b)<=REL*max(abs(a),abs(b))))
    if isinstance(a,dict): return a.keys()==b.keys() and all(equal(a[k],b[k],tolerant) for k in a)
    if isinstance(a,list): return len(a)==len(b) and all(equal(x,y,tolerant) for x,y in zip(a,b))
    return a==b
def csv_rows(path):
    with path.open(encoding='utf-8',newline='') as f:
        reader=csv.DictReader(f); fields=[k for k in reader.fieldnames if k!='run_id']
        rows=[]
        for row in reader:
            row={k:row[k] for k in fields}
            if row.get('value_kind')=='number' and row.get('value') not in (None,''): row['value']=Decimal(row['value'])
            rows.append(row)
    return fields,rows
def signature(path):
    result=load(path/'results.json'); manifest=load(path/'manifest.json')
    assert manifest['metadata_ref']=='results.json#/metadata'
    assert manifest['run_id']==result['run_id']
    assert manifest['status']=='complete' and manifest['partial'] is False
    assert manifest['termination']==result['simulation']['termination']
    for f in manifest['files']:
        p=path/f['name']; assert p.resolve().is_relative_to(path.resolve())
        assert p.stat().st_size==int(f['bytes']) and sha(p)==f['sha256']
    return {'simulation':result['simulation'], 'events':list(csv_rows(path/'events.csv')), 'summary':list(csv_rows(path/'summary.csv'))},result['metadata']
FINGERPRINT = ('git_commit','compiler','cargo_lock_sha256','build_source_sha256','input_sha256','config_sha256','model_registry_version','model_profile','seed','window_ps','runtime_version','time_resolution_ps','initial_state','initial_channel_state')
def fingerprint(meta):
    assert all(k in meta for k in FINGERPRINT)
    return {k:meta[k] for k in FINGERPRINT}
def expected(case, sig):
    requests=sig['simulation']['requests']
    # Sorting this independent SOF projection does not sort the compared observations.
    sent=sorted((r for r in requests if r['sof_ps'] is not None),key=lambda r:int(r['sof_ps']))
    ids=['a:0','b:0'] if case=='competition' else ['b:0','a:0','b:1']
    sof=[0,106000000] if case=='competition' else [0,100000000,206000000]
    assert len(requests)==len(ids)
    assert [r['request_id'] for r in sent]==ids
    assert [int(r['sof_ps']) for r in sent]==sof
    if case=='competition': assert [int(r['eof_ps']) for r in sent]==[100000000,200000000]
def capture(args):
    repo=args.repo.resolve(); binary=args.binary.resolve(); output=args.output.resolve()
    assert binary.is_file() and not output.exists(), 'use a new evidence directory'
    git=subprocess.run(['git','rev-parse','HEAD'],cwd=repo,text=True,capture_output=True,check=True).stdout.strip()
    assert git==TARGET,'source commit mismatch'
    status=subprocess.run(['git','status','--porcelain'],cwd=repo,text=True,capture_output=True,check=True).stdout
    assert not status,'working tree must be clean; put evidence outside checkout'
    output.mkdir(parents=True)
    fixture=repo/'docs/verification/fixtures/can'
    input_files=sorted(p for p in fixture.rglob('*') if p.is_file())
    inputs={p.relative_to(repo).as_posix():sha(p) for p in input_files}
    before=sha(binary);commands=[]
    for case in CASES:
        for n in range(3):
            dest=output/f'{case}-{n}'; cmd=[str(binary),'run','--config',str(fixture/f'{case}.ini'),'--output',str(dest)]
            start=time.monotonic(); proc=subprocess.run(cmd,cwd=repo,text=True,capture_output=True)
            (output/f'{case}-{n}.stdout').write_text(proc.stdout,encoding='utf-8');(output/f'{case}-{n}.stderr').write_text(proc.stderr,encoding='utf-8')
            commands.append({'argv':cmd,'exit_code':proc.returncode,'wall_seconds_reference_only':time.monotonic()-start})
            assert proc.returncode==0
            sig,meta=signature(dest);expected(case,sig)
            assert meta['git_commit']==TARGET and meta['git_dirty'] in (False,'false')
    assert before==sha(binary) and inputs=={p.relative_to(repo).as_posix():sha(p) for p in input_files}
    write(output/'capture.json',{'target_commit':TARGET,'environment_kind':args.environment,'os':platform.platform(),'cpu':platform.processor(),'python':platform.python_version(),'binary_sha256':before,'cargo_lock_sha256':sha(repo/'Cargo.lock'),'fixture_sha256':inputs,'commands':commands,'scope':'two CAN CLI fixtures; does not prove all TEST0083 scheduler/permutation or all ACs'})
    compare_series(output,False)
def compare_series(root,tolerant):
    capture=load(root/'capture.json');assert capture['target_commit']==TARGET
    for case in CASES:
        a,meta=signature(root/f'{case}-0');expected(case,a)
        for n in (1,2):
            b,other=signature(root/f'{case}-{n}');expected(case,b)
            assert equal(a,b,False),f'within-environment repeat mismatch {case}-{n}'
            assert equal(fingerprint(meta),fingerprint(other),False)
            assert meta['binary_sha256']==other['binary_sha256']
    return capture
def compare(args):
    a,b=args.left.resolve(),args.right.resolve(); ca,cb=compare_series(a,False),compare_series(b,False)
    assert {ca['environment_kind'],cb['environment_kind']}=={'native-ubuntu','wsl2-ubuntu'},'not an independent native/WSL pair'
    assert ca['fixture_sha256']==cb['fixture_sha256'] and ca['cargo_lock_sha256']==cb['cargo_lock_sha256'],'comparison conditions differ'
    for case in CASES:
        sa,ma=signature(a/f'{case}-0');sb,mb=signature(b/f'{case}-0')
        assert equal(fingerprint(ma),fingerprint(mb),False),'comparison conditions differ; do not classify as determinism failure'
        assert equal(sa,sb,True),f'cross-environment correctness mismatch: {case}'
    write(args.output,{'status':'passed','target_commit':TARGET,'cases':list(CASES),'repetitions_per_environment':3,'integer_time_order_count':'exact','derived_ratios':'absolute <=1e-12 OR relative <=1e-9','row_array_order':'preserved','left':str(a),'right':str(b),'binary_sha256':[ca['binary_sha256'],cb['binary_sha256']],'limits':['environment kind is operator assertion; attach uname, lscpu, kernel and virtualization evidence','fake models/EventKey/dirty resource and generator permutations require separate Rust evidence','all-profile and large-input correctness not covered']})
def selftest():
    assert equal({'a':[1,'2',Decimal('0.3')]},{'a':[1,'2',Decimal('0.3000000000001')]},True)
    assert not equal([1,2],[2,1],True)
    assert not equal(1,2,True)
    assert not equal('100','101',True)
    assert not equal(Decimal('0.3'),Decimal('0.3000000000001'),False)
    assert not equal(Decimal('0.3'),Decimal('0.31'),True)
    assert not equal(1,'1',True)
    assert equal(Decimal('0'),Decimal('1e-13'),True)
    assert not equal(Decimal('0'),Decimal('1e-10'),True)
    assert list(csv.DictReader(io.StringIO('run_id,seq\na,1\n')))[0]['seq']=='1'
    with tempfile.TemporaryDirectory() as d:
        a,b=Path(d)/'a.csv',Path(d)/'b.csv'
        a.write_text('run_id,seq,value_kind,value\na,0,number,0.3\na,1,integer,2\n',encoding='utf-8')
        b.write_text('run_id,seq,value_kind,value\nb,0,number,0.3000000000001\nb,1,integer,2\n',encoding='utf-8')
        assert equal(list(csv_rows(a)),list(csv_rows(b)),True)
        assert not equal(list(csv_rows(a)),list(csv_rows(b)),False)
        b.write_text('run_id,seq,value_kind,value\nb,1,integer,2\nb,0,number,0.3\n',encoding='utf-8')
        assert not equal(list(csv_rows(a)),list(csv_rows(b)),True)
        b.write_text('run_id,seq,value_kind,value\nb,0,number,0.3\nb,1,integer,3\n',encoding='utf-8')
        assert not equal(list(csv_rows(a)),list(csv_rows(b)),True)
    print('PASS: ordered typed comparison, integer exactness, ratio tolerance and zero boundary')
if __name__=='__main__':
    parser=argparse.ArgumentParser();sub=parser.add_subparsers(dest='command',required=True)
    c=sub.add_parser('capture');c.add_argument('--repo',type=Path,required=True);c.add_argument('--binary',type=Path,required=True);c.add_argument('--output',type=Path,required=True);c.add_argument('--environment',choices=['native-ubuntu','wsl2-ubuntu'],required=True)
    c=sub.add_parser('compare');c.add_argument('--left',type=Path,required=True);c.add_argument('--right',type=Path,required=True);c.add_argument('--output',type=Path,required=True)
    sub.add_parser('selftest'); args=parser.parse_args()
    if args.command=='capture': capture(args)
    elif args.command=='compare': compare(args)
    else: selftest()
