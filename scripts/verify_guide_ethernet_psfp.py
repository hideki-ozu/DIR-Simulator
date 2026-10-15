"""Measure only PSFP yellow_action using formal CLI and isolated public ZIP."""
import argparse,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ethernet-psfp'
COMMIT='b45515644dfc65c205216d564d5bf7ef00280622';CASES=('pass','drop')
GENERATED=(0,0,0,32000000,96000000);SOFS=(0,672000,1344000,32000000,96000000)
COLORS=('green','yellow','red','yellow','green')
# Independent fixed numerator expectations from the specified two-bucket arithmetic.
CB=(512000000000000,5376000000000,10752000000000,256000000000000,512000000000000)
PB=(1024000000000000,522752000000000,21504000000000,512000000000000,1024000000000000)
CA=(0,5376000000000,10752000000000,256000000000000,0)
PA=(512000000000000,10752000000000,21504000000000,0,512000000000000)

def check_inputs():
 baseline=json.loads((ROOT/'examples/ethernet/tsn/psfp-model.json').read_text());assert json.loads((SOURCE/'pass.json').read_text())==baseline
 baseline['tsn']['streams'][0]['meter']['yellow_action']='ACTION'
 for case in CASES:
  model=json.loads((SOURCE/(case+'.json')).read_text());assert model['tsn']['streams'][0]['meter']['yellow_action']==case
  model['tsn']['streams'][0]['meter']['yellow_action']='ACTION';assert model==baseline
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','MODEL.json')==(SOURCE/'pass.ini').read_text().replace('pass.json','MODEL.json')
 for local,original in [('models/ethtsn/Main.ned','models/ethtsn/Main.ned'),('workload.json','psfp-workload.json')]:assert (SOURCE/local).read_bytes()==(ROOT/'examples/ethernet/tsn'/original).read_bytes()

def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case in CASES:
  result=out/case;commands={}
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr;commands[cmd[0]]=json.loads(p.stdout)
  assert commands['validate']=={'channel_count':'2','network':'ethtsn.Net','node_count':'2','schema_version':1,'status':'valid'}
  assert commands['run']['exit_code']==0 and commands['run']['primary_diagnostic'] is None and commands['view']['status']=='complete'
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']==COMMIT and str(meta['git_dirty'])=='false'
  assert sim['end_ps']=='120000000' and sim['last_event_time_ps']=='96672000' and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  group=lambda name:[r for r in sim['model_records'] if r['schema_name']==name]
  frames=group('ethernet.dynamic.frame');transfers=group('ethernet.dynamic.transfer');receptions=group('ethernet.dynamic.reception');policing=group('ethernet.tsn.policing')
  assert len(frames)==len(transfers)==len(receptions)==len(policing)==5
  for n,(generated,sof,color) in enumerate(zip(GENERATED,SOFS,COLORS)):
   arrival=sof+576000;frame=next(r['data'] for r in frames if r['record_id']==f'psfp:{n}');assert frame['generated_ps']==str(generated) and frame['priority']=='7'
   t=next(r for r in transfers if r['record_id']==f'psfp:{n}/v0/c{n}');d=t['data']
   assert d['from_port']=='Net.a.tx' and d['to_port']=='Net.b.rx' and d['parent_transfer_id'] is None and d['queued_ps']==str(generated) and d['status']=='serialized' and d['drop_reason'] is None
   assert d['sof_ps']==str(sof) and d['eof_ps']==d['arrival_ps']==str(arrival) and d['release_ps']==str(sof+672000)
   assert d['planned_eof_ps']==d['eof_ps'] and d['planned_arrival_ps']==d['arrival_ps'] and d['planned_release_ps']==d['release_ps']
   w=d['wire'];raw=bytes.fromhex(w['mac_hex']);assert w['mac_bytes']=='64' and w['pad_bytes']=='46' and w['data_hex']=='' and w['tag'] is None
   assert len(raw)==64 and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little')
   p=next(r for r in policing if r['data']['reception_id']==t['record_id']+'@rx');q=p['data']
   verdict='psfp_meter_red' if color=='red' else 'psfp_meter_yellow' if color=='yellow' and case=='drop' else 'pass'
   assert p['time_ps']==str(arrival) and q['stream_id']=='s0' and q['ingress']=='Net.b.rx' and q['mac_bytes']=='64' and q['color']==color and q['verdict']==verdict
   assert [q[k] for k in ('committed_before','peak_before','committed_after','peak_after')]==list(map(str,(CB[n],PB[n],CA[n],PA[n])))
   assert q['consumed_bits']==str({'green':1024,'yellow':512,'red':0}[color])
   rx=next(r['data'] for r in receptions if r['data']['transfer_id']==t['record_id']);accepted=verdict=='pass'
   assert rx['status']==('received' if accepted else 'filtered') and rx['reason']==(None if accepted else verdict) and rx['observed_ps']==rx['ready_ps']==str(arrival)
   assert rx['planned_ready_ps']==(str(arrival) if accepted else None) and rx['egress_transfer_ids']==[]
  s={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  for metric,v in {'generated':5,'serialized':5,'received':4 if case=='pass' else 2,'filtered':1 if case=='pass' else 3,'dropped':0,'queued':0,'transmitting':0,'processing':0}.items():assert s['$all','ethernet.'+metric]==str(v)
  assert s['$all','ethernet.filtered.psfp_meter_red']=='1' and s['$all','ethernet.filtered.psfp_meter_yellow']==('0' if case=='pass' else '2')
  assert s['Net.a.tx','ethernet.wire_bits']=='2880' and s['Net.a.tx','ethernet.occupied_bits']=='3360'
  assert not group('ethernet.tsn.credit')
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','compiler','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 # Changing acceptance does not refund tokens or alter source serialization.
 for schema,keys in [('ethernet.tsn.policing',('reception_id','color','committed_before','peak_before','committed_after','peak_after','consumed_bits')),('ethernet.dynamic.transfer',('frame_id','sof_ps','eof_ps','arrival_ps','release_ps','status','wire'))]:
  projection=lambda sim:sorted((tuple(json.dumps(r['data'][k],sort_keys=True) for k in keys) for r in sim['model_records'] if r['schema_name']==schema))
  assert projection(values['pass'])==projection(values['drop'])
 return values,provenance

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 public,prov=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-psfp-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==6 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-psfp',out/'zip');assert public==zipped
 evidence={'generated_by':'scripts/verify_guide_ethernet_psfp.py','status':'passed','source_tag':'v1.1.4','source_commit':COMMIT,'cli_operations':12,'zip_input_count':6,'single_variable':'tsn.streams[0].meter.yellow_action','zip_reproduction':'all simulation objects identical','public_input_sha256':{p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCE.rglob('*') if p.is_file()},'conditions':public,'provenance':prov,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ethernet-psfp';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS: 12 CLI operations; yellow_action only, arrival-based two-bucket colors/tokens, no refund, wire timing/CRC and isolated ZIP')
if __name__=='__main__':main()
