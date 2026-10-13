"""Measure formal AHB wait/ERROR cases and reproduce them from the public ZIP."""
import argparse,hashlib,json,subprocess,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ahb-wait-error';CASES=('wait0','wait1','wait2')
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case in CASES:
  wait=int(case[-1]);result=out/case;commands={}
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr;x=json.loads(p.stdout);commands[cmd[0]]=x
  assert commands['validate']=={'channel_count':'0','network':'demo.Main','node_count':'4','schema_version':1,'status':'valid'}
  assert commands['run']['exit_code']==0 and commands['run']['primary_diagnostic'] is None and commands['view']['status']=='complete'
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation']
  assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622' and meta['git_dirty'] in (False,'false','clean')
  first=(2+wait)*10;last=(5+2*wait)*10
  assert sim['end_ps']=='100' and sim['last_event_time_ps']==str(last) and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  records={r['request_id']:r for r in sim['model_records'] if r['schema_name']=='ahb.transaction'};assert set(records)=={'a:0','b:0'}
  for rid,start,end,response,address,manager in [('a:0',0,first,'OKAY',0,'Main.m0'),('b:0',first,last,'ERROR',16,'Main.m1')]:
   r=records[rid];d=r['data'];assert r['time_ps']==str(end)
   assert d['generated_ps']=='0' and d['start_ps']==str(start) and d['completed_ps']==str(end)
   assert d['status']=='completed' and d['response']==response and d['drop_reason'] is None and d['active_plan'] is None
   assert d['operation']=='read' and d['address']==str(address) and d['bytes']=='4' and d['target']=='Main.t' and d['source']==manager
  transfers={r['request_id']:r for r in sim['model_records'] if r['schema_name']=='ahb.transfer'};assert set(transfers)==set(records)
  for rid,r in transfers.items():
   d=r['data'];tx=records[rid]['data'];assert r['subject']=='Main.bus' and d['from']==tx['source'] and d['to']=='Main.t'
   assert d['start_ps']==tx['start_ps'] and d['end_ps']==tx['completed_ps'] and d['bytes']=='4' and d['response']==tx['response'] and d['hop']=='0' and r['time_ps']==d['end_ps']
  assert sum(int(r['data']['end_ps'])-int(r['data']['start_ps']) for r in transfers.values())==last
  summary={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  assert summary['$all','ahb.generated']==summary['$all','ahb.completed']=='2' and summary['$all','ahb.errors']=='1' and summary['$all','ahb.delivered_bits']=='32'
  for k in ['ahb.dropped','ahb.pending','ahb.active']:assert summary['$all',k]=='0'
  assert summary['Main.bus','ahb.utilization']==last/100 and summary['$all','ahb.wait_mean_ps']==first/2 and summary['$all','ahb.latency_mean_ps']==(first+last)/2
  assert summary['Main.m0','ahb.wait_mean_ps']==0 and summary['Main.m1','ahb.wait_mean_ps']==first
  assert summary['Main.m0','ahb.delivered_bits']=='32' and summary['Main.m1','ahb.delivered_bits']=='0'
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 base=json.loads((SOURCE/'wait0.json').read_text());base['targets'][0].pop('wait_cycles')
 for case in CASES:
  model=json.loads((SOURCE/(case+'.json')).read_text());assert model['targets'][0].pop('wait_cycles')==int(case[-1]) and model==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','MODEL.json')==(SOURCE/'wait0.ini').read_text().replace('wait0.json','MODEL.json')
 public,provenance=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ahb-wait-error-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==8 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ahb-wait-error',out/'zip');assert public==zipped
 evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':len(expected),'single_variable':'targets[0].wait_cycles','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':provenance,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ahb-wait-error';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS: 18 CLI operations; one-variable target wait, OKAY/ERROR completed, timing/32bit/busy, manifest and isolated ZIP')
if __name__=='__main__':main()
