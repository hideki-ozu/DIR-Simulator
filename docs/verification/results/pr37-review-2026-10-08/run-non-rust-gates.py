from pathlib import Path
import hashlib, json, os, re, subprocess

BASE = Path('/tmp/dir-pr37-review-2026-10-08')
REPO = Path('/tmp/dir-v1.1.4-preparation-2026-10-08/worktree')
BIN = Path('/tmp/dir-v1.1.4-preparation-2026-10-08/target/release/dir-simulator')
OUT = BASE / 'non-rust-gates'
OUT.mkdir()
(OUT / 'logs').mkdir()
(OUT / 'products').mkdir()
(OUT / 'tmp').mkdir()
env = os.environ.copy()
env.update({'DIR_SIMULATOR_BIN':str(BIN), 'CARGO':'/home/hideki/.cargo/bin/cargo', 'CARGO_TARGET_DIR':'/tmp/dir-v1.1.4-preparation-2026-10-08/target', 'NODE_PATH':'/home/hideki/.npm/_npx/705bc6b22212b352/node_modules', 'TMPDIR':str(OUT / 'tmp')})
NODE = '/home/hideki/.nvm/versions/node/v22.22.0/bin/node'
record = {'binary_sha256':hashlib.sha256(BIN.read_bytes()).hexdigest(), 'commands':[], 'runs':[]}

def call(name, argv, expected=0):
    log = OUT / 'logs' / (name + '.log')
    with log.open('wb') as f:
        result = subprocess.run(argv, cwd=REPO, env=env, stdout=f, stderr=subprocess.STDOUT)
    row = {'name':name, 'argv':list(map(str,argv)), 'exit_code':result.returncode, 'expected_exit_code':expected, 'log':str(log.relative_to(OUT)), 'sha256':hashlib.sha256(log.read_bytes()).hexdigest()}
    record['commands'].append(row)
    print(json.dumps(row), flush=True)
    if result.returncode != expected:
        print(log.read_text()[-7000:], flush=True)
        record['status']='failed'
        (OUT / 'gate.json').write_text(json.dumps(record, indent=2)+'\n')
        raise SystemExit(1)
    return log

prior = json.loads(Path('/tmp/dir-v1.1.4-preparation-2026-10-08/non-rust-gates/simulator-runs.json').read_text())
products = {}
for case in prior['runs']:
    if case['name']=='tsn-cap20-calibration':
        continue
    name = case['name']
    dest = OUT / 'products' / name
    call(name, [str(BIN),'run','--config',case['argv'][3],'--output',str(dest)], case['exit_code'])
    raw = json.loads((dest / 'results.json').read_text())
    assert raw['simulation']['partial'] == (case['exit_code']==3), name
    manifest = json.loads((dest/'manifest.json').read_text())
    entries=manifest['files']
    if isinstance(entries,dict):entries=[dict(v,path=k) for k,v in entries.items()]
    for entry in entries:
        f=dest/entry.get('path',entry.get('name'))
        assert f.stat().st_size==int(entry.get('bytes',entry.get('size_bytes'))),f
        assert hashlib.sha256(f.read_bytes()).hexdigest()==entry['sha256'],f
    products[name] = dest / 'results.json'
    record['runs'].append({'name':name,'input':case['argv'][3],'expected_exit_code':case['exit_code'],'partial':raw['simulation']['partial'],'result':str(products[name].relative_to(OUT)),'sha256':hashlib.sha256(products[name].read_bytes()).hexdigest(),'manifest_files_verified':len(entries)})

cap = OUT / 'products' / 'capability'
call('capability-run',[str(BIN),'run','--config',str(REPO/'docs/verification/fixtures/acceptance-2026-10-08/dynamic-tsn/psfp-queued-membership-leave.ini'),'--output',str(cap)])
call('capability-view',[str(BIN),'view','--input',str(cap/'results.json'),'--output',str(cap/'viewer.html')])
env['DIR_VIEWER_CAPABILITY_RESULT']=str(cap/'results.json')
node_log=call('node-tests',[NODE,'--test',*map(str,sorted((REPO/'tests').glob('*.test.cjs')))])
record['node_counts']={k:int(v) for k,v in re.findall(r'^# (tests|pass|fail|skipped) (\d+)$',node_log.read_text(),re.M)}
assert record['node_counts']['fail']==0 and record['node_counts']['skipped']==0
call('network-browser',[NODE,'tests/network_viewer_browser.cjs',*map(str,[products[k] for k in ['bridge-complete','dynamic-complete','tsn-complete','bridge-partial','dynamic-partial','tsn-partial-tight']])])
call('capability-browser',[NODE,'tests/network_viewer_browser.cjs','--exported-html',str(cap/'results.json'),str(cap/'viewer.html')])
call('can-gateway-browser',[NODE,'tests/viewer_browser.cjs'])
call('transaction-browser',[NODE,'tests/transaction_viewer_browser.cjs',*map(str,[products[k] for k in ['axi-complete','soc-complete','memory-complete','ahb-rejection','memory-zero-rejection']])])
record['status']='passed'
(OUT/'gate.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({'status':'passed','node_counts':record['node_counts'],'product_runs':len(record['runs']),'browser_groups':4}),flush=True)
