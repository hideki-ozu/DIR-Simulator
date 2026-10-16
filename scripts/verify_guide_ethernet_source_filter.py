"""Measure source INCLUDE/EXCLUDE as the only variable with formal CLI and isolated ZIP."""
import argparse,copy,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ethernet-source-filter';COMMIT='b45515644dfc65c205216d564d5bf7ef00280622';CASES=('include','exclude')
def load(p):return json.loads(p.read_text(encoding='utf-8'))
def check_inputs():
 public=load(ROOT/'examples/ethernet/dynamic/membership.json');expected={'schema_version':4,'generators':public['generators'][:1],'controls':public['controls'][:1]};expected['generators'][0]['node']='Main.a';expected['controls'][0].update(switch='Main.sw',port='Main.sw.tx_b')
 base=load(SOURCE/'include.json');assert base==expected
 for case in CASES:
  w=load(SOURCE/(case+'.json'));assert w['controls'][0]['mode']==case;w['controls'][0]['mode']='include';assert w==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','WORKLOAD.json')==(SOURCE/'include.ini').read_text().replace('include.json','WORKLOAD.json')
 model=load(SOURCE/'model.json');prior=load(ROOT/'examples/guide/ethernet-link-control/model.json');endpoint=load(ROOT/'examples/ethernet/dynamic/model.json')['endpoints']
 for e,pub in zip(prior['endpoints'],endpoint):e['multicast']=pub['multicast']
 assert model==prior and model['switches'][0]['multicast']==[] and model['switches'][0]['unknown_multicast']=='flood'
 assert (SOURCE/'models/ethdemo/Main.ned').read_bytes()==(ROOT/'examples/ethernet/models/ethdemo/Main.ned').read_bytes()
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case in CASES:
  result=out/case
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr
   if cmd[0]=='validate':assert json.loads(p.stdout)=={'channel_count':'6','network':'ethdemo.Main','node_count':'4','schema_version':1,'status':'valid'}
  manifest=load(result/'manifest.json');assert manifest['status']=='complete' and manifest['partial'] is False
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=load(result/'results.json');meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']==COMMIT and str(meta['git_dirty'])=='false' and meta['binary_sha256']==hashlib.sha256(binary.read_bytes()).hexdigest()
  assert sim['end_ps']=='10000000' and sim['last_event_time_ps']=='4315000' and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  group=lambda n:[r for r in sim['model_records'] if r['schema_name']=='ethernet.dynamic.'+n]
  assert len(group('frame'))==2;tx=group('transfer');rx=group('reception');assert len(tx)==len(rx)==(5 if case=='include' else 4)
  expected=[('ipv4:0','Main.a.tx','Main.sw.rx_a',100),('ipv4:1','Main.a.tx','Main.sw.rx_a',3000000),('ipv4:1','Main.sw.tx_b','Main.b.rx',3611000),('ipv4:1','Main.sw.tx_c','Main.c.rx',3611000)]
  if case=='include':expected.append(('ipv4:0','Main.sw.tx_b','Main.b.rx',611100))
  for frame,port,peer,sof in expected:
   t=next(r for r in tx if r['data']['frame_id']==frame and r['data']['from_port']==port);d=t['data'];assert d['to_port']==peer and d['status']=='serialized' and d['drop_reason'] is None
   assert [d[x] for x in ('sof_ps','eof_ps','release_ps','arrival_ps')]==list(map(str,(sof,sof+608000,sof+704000,sof+609000)))
   if port=='Main.a.tx':assert d['parent_transfer_id'] is None
   else:assert d['parent_transfer_id']==next(r['record_id'] for r in tx if r['data']['frame_id']==frame and r['data']['from_port']=='Main.a.tx')
   raw=bytes.fromhex(d['wire']['mac_hex']);assert len(raw)==68 and raw[:6].hex()=='01005e010101' and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little')
   reception=next(r['data'] for r in rx if r['data']['transfer_id']==t['record_id']);assert reception['observed_ps']==d['arrival_ps']
   filtered=case=='exclude' and frame=='ipv4:0';assert reception['status']==('filtered' if filtered else 'forwarded' if peer=='Main.sw.rx_a' else 'received');assert reception['reason']==('multicast_filtered' if filtered else None)
   if peer=='Main.sw.rx_a':assert reception['ready_ps']==str(sof+611000) and len(reception['egress_transfer_ids'])==(0 if filtered else 1 if frame=='ipv4:0' else 2) and reception['policy_epoch_ingress']==('1' if frame=='ipv4:0' else '2')
  controls=group('control');member=sorted([r for r in controls if r['data']['kind'].startswith('membership_')],key=lambda r:int(r['time_ps']));assert len(controls)==4 and len(member)==2
  for r,time,kind in zip(member,(0,2000000),('membership_set','membership_expire')):
   d=r['data'];assert r['time_ps']==d['scheduled_ps']==d['applied_ps']==str(time) and d['kind']==kind and d['outcome']=='changed';assert d['key']=={'family':'ipv4','group':'239.1.1.1','port':'Main.sw.tx_b','switch':'Main.sw','vid':'10'}
  assert member[0]['data']['after']==member[1]['data']['before']=={'expires_at':'2000000','generation':'1','value':{'mode':case,'sources':['192.0.2.1']}} and member[1]['data']['after'] is None
  policies=group('policy');assert len(policies)==3 and all(p['data']['topology_generation']=='0' for p in policies)
  summary={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  for metric,v in {'generated':2,'serialized':len(tx),'received':3 if case=='include' else 2,'forwarded':2 if case=='include' else 1,'filtered':0 if case=='include' else 1,'dropped':0,'queued':0,'transmitting':0,'processing':0}.items():assert summary['$all','ethernet.'+metric]==str(v)
  if case=='exclude':assert summary['$all','ethernet.filtered.multicast_filtered']=='1'
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','compiler','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False);values,prov=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-source-filter-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==6 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-source-filter',out/'zip');assert values==zipped
 report={'generated_by':'scripts/verify_guide_ethernet_source_filter.py','status':'passed','source_tag':'v1.1.4','source_commit':COMMIT,'cli_operations':12,'zip_input_count':6,'single_variable':'controls[0].mode after common normalization','zip_reproduction':'all simulation objects identical','public_input_sha256':{p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCE.rglob('*') if p.is_file()},'conditions':values,'provenance':prov,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ethernet-source-filter';dest.mkdir(parents=True,exist_ok=True);raw=(json.dumps(report,ensure_ascii=False,indent=2)+'\n').encode();(dest/'measurements.json').write_bytes(raw);(out/'measurements.json').write_bytes(raw)
 print('PASS: 12 formal CLI operations, one mode variable, full copy timing/CRC/parent paths, known mismatch/no flood, expiry and isolated ZIP equality')
if __name__=='__main__':main()
