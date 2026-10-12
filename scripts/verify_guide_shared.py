"""Measure shared-memory consumer roles using formal CLI and an isolated public ZIP."""
import argparse,hashlib,json,subprocess,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/shared-roles';CASES=('reader','writer','guest')
def measure(binary,source,output):
 output.mkdir(parents=True);values={};provenance={}
 for actor in CASES:
  result=output/actor
  for command in [['validate','--config',str(source/(actor+'.ini'))],['run','--config',str(source/(actor+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(output/(actor+'-viewer.html'))]]:
   run=subprocess.run([str(binary),*command],capture_output=True,text=True);assert run.returncode==0,run.stderr
   parsed=json.loads(run.stdout);assert parsed.get('status') in ('valid','complete') or parsed.get('exit_code')==0
   if command[0]=='validate':assert parsed['node_count']=='1' and parsed['channel_count']=='0' and parsed['network']=='shared.Main'
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation'];allowed=actor=='reader'
  assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622' and meta['git_dirty'] in (False,'false','clean')
  assert sim['end_ps']=='400' and sim['termination']=='events_exhausted' and sim['pending_events']=='0' and not sim['partial']
  assert sim['last_event_time_ps']==('250' if allowed else '150')
  rows={x['request_id']:x for x in sim['model_records'] if x['schema_name']=='memory-ipc.request'};assert set(rows)=={'publish:0','consume:0'}
  pub=rows['publish:0']['data'];assert pub['actor']=='writer' and pub['op']=='publish' and pub['status']=='completed' and pub['reason']=='ok'
  assert pub['generated_ps']==pub['started_ps']=='0' and pub['completed_ps']==pub['planned_completion_ps']=='100' and pub['input_hex']=='01020304' and pub['slot']==0
  con=rows['consume:0']['data'];assert con['actor']==actor and con['op']=='consume' and con['generated_ps']=='150'
  assert con['status']==('completed' if allowed else 'rejected') and con['reason']==('ok' if allowed else 'access_denied')
  assert con['started_ps']==('150' if allowed else None) and con['completed_ps']==('250' if allowed else '150')
  assert con['planned_completion_ps']==('250' if allowed else None) and con['slot']==(0 if allowed else None)
  assert con['output_hex']==('01020304' if allowed else None) and con['dispatch_ordinal']==('1' if allowed else None)
  for row in rows.values():assert row['subject']=='Main.buf' and row['origin_request_id'] is None and row['data']['model']=='shared'
  shared=next(x['data'] for x in sim['model_records'] if x['schema_name']=='memory-ipc.shared')
  assert shared['active'] is None and shared['queue']==[] and shared['ready']==([] if allowed else [0])
  assert shared['slots']==[{'hex':'' if allowed else '01020304','index':0,'message_id':None if allowed else 'publish:0','owner':None,'state':'free' if allowed else 'ready'}]
  summary={x['metric']:x['value'] for x in sim['summary']};assert summary['memory_ipc.offered']=='2' and summary['memory_ipc.committed_bytes']=='4'
  assert summary['memory_ipc.completed']==('2' if allowed else '1') and summary['memory_ipc.rejected']==('0' if allowed else '1')
  assert abs(summary['memory_ipc.busy_ratio']-(.5 if allowed else .25))<1e-12 and summary['memory_ipc.queue_mean']==0
  values[actor]=sim;provenance[actor]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')}
  provenance[actor]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 # Producer role and unregistered actor are both refused without altering published payload.
 def sans_actor(sim):
  value=json.loads(json.dumps(sim))
  for rec in value['model_records']:
   if rec['request_id']=='consume:0':rec['data']['actor']='ROLE'
  return value
 assert sans_actor(values['writer'])==sans_actor(values['guest'])
 assert all(next(x for x in sim['model_records'] if x['request_id']=='publish:0')==next(x for x in values['reader']['model_records'] if x['request_id']=='publish:0') for sim in values.values())
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 base=json.loads((SOURCE/'reader.json').read_text());base['generators'][1]['request'].pop('actor')
 for actor in CASES:
  w=json.loads((SOURCE/(actor+'.json')).read_text());assert w['generators'][1]['request'].pop('actor')==actor and w==base
  assert (SOURCE/(actor+'.ini')).read_text().replace(actor+'.json','WORKLOAD.json')==(SOURCE/'reader.ini').read_text().replace('reader.json','WORKLOAD.json')
 public,provenance=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/shared-roles-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/shared-roles',out/'zip');assert public==zipped
 evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':len(expected),'single_variable':'generators[1].request.actor','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':provenance,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/shared-roles';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS: 18 CLI operations; actor-only comparison, publication/consume, refusal/retained bytes, metrics, manifest and isolated ZIP')
if __name__=='__main__':main()
