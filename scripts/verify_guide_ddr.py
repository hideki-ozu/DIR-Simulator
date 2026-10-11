"""Measure DDR refresh duration with formal CLI and an isolated public ZIP."""
import argparse,hashlib,json,subprocess,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
SOURCE=ROOT/'examples/guide/ddr-refresh'
CASES=(50,100,200)
def measure(binary,source,output):
 output.mkdir(parents=True);values={};provenance={}
 for delay in CASES:
  name=f'refresh{delay}';result=output/name
  for command in [['validate','--config',str(source/(name+'.ini'))],['run','--config',str(source/(name+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(output/(name+'-viewer.html'))]]:
   run=subprocess.run([str(binary),*command],capture_output=True,text=True);assert run.returncode==0,run.stderr
   parsed=json.loads(run.stdout);assert parsed.get('status') in ('valid','complete') or parsed.get('exit_code')==0
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for entry in manifest['files']:
   raw=(result/entry['name']).read_bytes();assert str(len(raw))==entry['bytes'] and hashlib.sha256(raw).hexdigest()==entry['sha256']
  document=json.loads((result/'results.json').read_text());meta,sim=document['metadata'],document['simulation']
  assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622' and meta['git_dirty'] in (False,'false','clean')
  assert sim['end_ps']=='1000' and sim['termination']=='time_limit' and sim['pending_events']=='1' and not sim['partial']
  assert sim['last_event_time_ps']==str(750+delay)
  requests={x['request_id']:x for x in sim['model_records'] if x['schema_name']=='memory-ipc.request'};assert len(requests)==3
  for index,generated,start,end,hit in [(0,0,0,200,False),(1,450,450,550,True),(2,500,550+delay,750+delay,False)]:
   rec=requests[f'read:{index}'];d=rec['data'];assert rec['subject']=='Main.mem' and rec['origin_request_id'] is None
   assert d['model']=='ddr' and d['op']=='read' and d['status']=='completed' and d['reason']=='ok'
   assert d['generated_ps']==str(generated) and d['started_ps']==str(start) and d['completed_ps']==d['planned_completion_ps']==str(end)
   assert d['address']=='0' and d['length']=='4' and d['bank']==d['row']==0 and d['row_hit']==hit
   assert d['output_hex']=='01020304' and d['committed_bytes']=='0' and d['dispatch_ordinal']==str(index)
  memory=next(x['data'] for x in sim['model_records'] if x['schema_name']=='memory-ipc.memory')
  assert memory['hex']=='0102030405060708090a0b0c0d0e0f10'+'00'*48
  assert memory['size']=='64' and memory['kind']=='ddr' and memory['open_rows']==[0] and memory['ports']==[None] and memory['queue']==[]
  assert memory['refresh_started_ps']=='550' and memory['refresh_ended_ps']==memory['refresh_planned_end_ps']==str(550+delay) and not memory['refresh_pending']
  summary={x['metric']:x['value'] for x in sim['summary']};assert summary['memory_ipc.offered']==summary['memory_ipc.completed']=='3'
  assert summary['memory_ipc.rejected']==summary['memory_ipc.committed_bytes']=='0' and summary['memory_ipc.ddr_refreshes']=='1'
  assert abs(summary['memory_ipc.busy_ratio']-.5)<1e-12 # request service only: refresh excluded
  assert abs(summary['memory_ipc.queue_mean']-(50+delay)/1000)<1e-12
  values[name]=sim
  provenance[name]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')}
  provenance[name]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 # Initial two requests and all read payloads are invariant.
 for sim in values.values():
  initial=[x for x in sim['model_records'] if x['request_id'] in ('read:0','read:1')]
  reference=[x for x in values['refresh50']['model_records'] if x['request_id'] in ('read:0','read:1')]
  assert initial==reference
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
 output=a.output.resolve();output.mkdir(parents=True,exist_ok=False)
 base=json.loads((SOURCE/'refresh50.json').read_text());base['ddr'][0].pop('refresh_ps')
 for delay in CASES:
  name=f'refresh{delay}';v=json.loads((SOURCE/(name+'.json')).read_text());assert v['ddr'][0].pop('refresh_ps')==str(delay) and v==base
  assert (SOURCE/(name+'.ini')).read_text().replace(name+'.json','MODEL.json')==(SOURCE/'refresh50.ini').read_text().replace('refresh50.json','MODEL.json')
 public,provenance=measure(a.binary.resolve(),SOURCE,output/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ddr-refresh-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(output/'isolated')
 zipped,zprov=measure(a.binary.resolve(),output/'isolated/examples/guide/ddr-refresh',output/'zip');assert public==zipped
 evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':len(expected),'single_variable':'ddr[0].refresh_ps','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':provenance,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ddr-refresh';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n'
 (dest/'measurements.json').write_text(raw);(output/'measurements.json').write_text(raw)
 print('PASS: 18 CLI operations; DDR refresh/service separation, row hits, request times, payloads, manifest and isolated ZIP')
if __name__=='__main__':main()
