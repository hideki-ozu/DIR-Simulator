"""Measure NoC input-FIFO capacities using formal CLI and isolated public ZIP."""
import argparse,hashlib,json,subprocess,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/noc-input-fifo';CASES=('capacity1','capacity2','capacity3');GRANTS={1:[0,40,50,60],2:[0,10,40,50],3:[0,10,20,40]}
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case in CASES:
  capacity=int(case[-1]);result=out/case;commands={}
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr;commands[cmd[0]]=json.loads(p.stdout)
  assert commands['validate']=={'channel_count':'0','network':'demo.Main','node_count':'4','schema_version':1,'status':'valid'}
  assert commands['run']['exit_code']==0 and commands['run']['primary_diagnostic'] is None and commands['view']['status']=='complete'
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622' and meta['git_dirty'] in (False,'false','clean')
  assert sim['end_ps']=='90' and sim['last_event_time_ps']=='80' and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  transactions={r['request_id']:r for r in sim['model_records'] if r['schema_name']=='noc.transaction'};assert set(transactions)=={'a:0','a:1','a:2','a:3','b:0'}
  transfers={r['record_id']:r for r in sim['model_records'] if r['schema_name']=='noc.transfer'};assert len(transfers)==9
  for ordinal in range(4):
   rid=f'a:{ordinal}';r=transactions[rid];d=r['data'];start=GRANTS[capacity][ordinal];end=50+ordinal*10
   assert d=={'active_plan':None,'address':None,'bytes':'4','completed_ps':str(end),'drop_reason':None,'generated_ps':str(ordinal*10),'operation':None,'response':'OKAY','source':'Main.e00','start_ps':str(start),'status':'completed','target':'Main.e10'} and r['time_ps']==str(end)
   for hop,subject,origin,target,g,h,response in [(0,'Main.r00:out_east','Main.r00','Main.r10',start,start+10,None),(1,'Main.r10:local_out','Main.r10','Main.e10',40+ordinal*10,end,'OKAY')]:
    t=transfers[f'{rid}:{hop}'];assert t['request_id']==rid and t['subject']==subject and t['time_ps']==str(h)
    assert t['data']=={'bytes':'4','end_ps':str(h),'from':origin,'hop':str(hop),'response':response,'start_ps':str(g),'to':target}
   assert start+10<=40+ordinal*10
  b=transactions['b:0'];assert b['data']=={'active_plan':None,'address':None,'bytes':'16','completed_ps':'40','drop_reason':None,'generated_ps':'0','operation':None,'response':'OKAY','source':'Main.e10','start_ps':'0','status':'completed','target':'Main.e10'} and b['time_ps']=='40'
  t=transfers['b:0:0'];assert t['subject']=='Main.r10:local_out' and t['data']=={'bytes':'16','end_ps':'40','from':'Main.r10','hop':'0','response':'OKAY','start_ps':'0','to':'Main.e10'}
  s={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  assert s['$all','noc.generated']==s['$all','noc.completed']=='5' and s['$all','noc.delivered_bits']=='256'
  for metric in ['noc.errors','noc.dropped','noc.pending','noc.active']:assert s['$all',metric]=='0'
  assert s['Main.e00','noc.wait_mean_ps']=={1:22.5,2:10,3:2.5}[capacity] and s['Main.e00','noc.latency_mean_ps']==50
  assert s['$all','noc.wait_mean_ps']=={1:18,2:8,3:2}[capacity] and s['$all','noc.latency_mean_ps']==48
  assert s['Main.e10','noc.wait_mean_ps']==0 and s['Main.e10','noc.latency_mean_ps']==40 and s['Main.e00','noc.delivered_bits']==s['Main.e10','noc.delivered_bits']=='128'
  assert s['Main.r00:out_east','noc.utilization']==40/90 and s['Main.r10:local_out','noc.utilization']==80/90
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 base=json.loads((SOURCE/'capacity1.json').read_text());base.pop('input_capacity')
 assert base['source_capacity']==4 and base['clock_period']=='10ps' and base['bytes_per_cycle']==4 and base['link_cycles']==0 and base['columns']==2 and base['rows']==1
 for case in CASES:
  model=json.loads((SOURCE/(case+'.json')).read_text());assert model.pop('input_capacity')==int(case[-1]) and model==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','MODEL.json')==(SOURCE/'capacity1.ini').read_text().replace('capacity1.json','MODEL.json')
 public,provenance=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/noc-input-fifo-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==8 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/noc-input-fifo',out/'zip');assert public==zipped
 evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':8,'single_variable':'input_capacity','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':provenance,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/noc-input-fifo';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS: 18 CLI operations; input_capacity only, 5 transactions/9 transfers, grants/wait/latency/256bit, manifest and isolated ZIP')
if __name__=='__main__':main()
