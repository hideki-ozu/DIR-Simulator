"""Verify one-ps link restoration comparison with formal CLI and isolated ZIP."""
import argparse,copy,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
SOURCE=ROOT/'examples/guide/ethernet-link-control'
COMMIT='b45515644dfc65c205216d564d5bf7ef00280622'
CASES={'on-time':609000,'late':609001}
def load(p):return json.loads(p.read_text(encoding='utf-8'))
def check_inputs():
 base=load(SOURCE/'on-time.json');public=load(ROOT/'examples/ethernet/dynamic/topology.json')
 expected=copy.deepcopy(public);expected['generators'][0]['node']='Main.a';expected['controls'][0].update(at_ps='500000',link='a-sw');expected['controls'][1].update(at_ps='609000',link='a-sw');assert base==expected
 for case,time in CASES.items():
  w=load(SOURCE/(case+'.json'));assert w['controls'][1]['at_ps']==str(time);w['controls'][1]['at_ps']='609000';assert w==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','WORKLOAD.json')==(SOURCE/'on-time.ini').read_text().replace('on-time.json','WORKLOAD.json')
 assert (SOURCE/'models/ethdemo/Main.ned').read_bytes()==(ROOT/'examples/ethernet/models/ethdemo/Main.ned').read_bytes()
 model=load(SOURCE/'model.json');prior=load(ROOT/'examples/guide/ethernet-vlan/all.json');common=copy.deepcopy(model);common.pop('dynamic');common['schema_version']=3;assert common==prior
 dynamic=model['dynamic'];assert dynamic['convergence_ps']=='0' and len(dynamic['bridges'])==1 and len(dynamic['links'])==3 and all(x['up'] for x in dynamic['links'])
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case,up in CASES.items():
  result=out/case
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr
   if cmd[0]=='validate':assert json.loads(p.stdout)=={'channel_count':'6','network':'ethdemo.Main','node_count':'4','schema_version':1,'status':'valid'}
  manifest=load(result/'manifest.json');assert manifest['status']=='complete' and manifest['partial'] is False
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=load(result/'results.json');meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']==COMMIT and str(meta['git_dirty'])=='false' and meta['binary_sha256']==hashlib.sha256(binary.read_bytes()).hexdigest()
  assert sim['end_ps']=='10000000' and sim['last_event_time_ps']=='3315000' and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  group=lambda n:[r for r in sim['model_records'] if r['schema_name']=='ethernet.dynamic.'+n]
  assert len(group('frame'))==2;tx=group('transfer');rx=group('reception');assert len(tx)==len(rx)==(4 if case=='on-time' else 3)
  expected=[('unicast:0','Main.a.tx','Main.sw.rx_a',0),('unicast:1','Main.a.tx','Main.sw.rx_a',2000000),('unicast:1','Main.sw.tx_b','Main.b.rx',2611000)]
  if case=='on-time':expected.append(('unicast:0','Main.sw.tx_b','Main.b.rx',611000))
  for frame,port,peer,sof in expected:
   t=next(r for r in tx if r['data']['frame_id']==frame and r['data']['from_port']==port);d=t['data'];assert d['to_port']==peer and d['status']=='serialized' and d['drop_reason'] is None
   assert [d[x] for x in ('sof_ps','eof_ps','release_ps','arrival_ps')]==list(map(str,(sof,sof+608000,sof+704000,sof+609000)))
   if port=='Main.a.tx':assert d['parent_transfer_id'] is None
   else:assert d['parent_transfer_id']==next(r['record_id'] for r in tx if r['data']['frame_id']==frame and r['data']['from_port']=='Main.a.tx')
   raw=bytes.fromhex(d['wire']['mac_hex']);assert len(raw)==68 and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little')
   reception=next(r['data'] for r in rx if r['data']['transfer_id']==t['record_id']);assert reception['observed_ps']==d['arrival_ps']
   filtered=case=='late' and frame=='unicast:0';assert reception['status']==('filtered' if filtered else 'forwarded' if peer=='Main.sw.rx_a' else 'received');assert reception['reason']==('link_down' if filtered else None)
   if filtered:assert reception['ready_ps']=='609000' and reception['planned_ready_ps'] is None and reception['egress_transfer_ids']==[] and reception['policy_epoch_ingress']=='1'
   elif peer=='Main.sw.rx_a':assert reception['ready_ps']==str(sof+611000) and len(reception['egress_transfer_ids'])==1 and reception['policy_epoch_ingress']=='2'
  control=group('control');links=[r for r in control if r['data']['kind']=='link_set'];assert len(links)==2
  for r,time,before,after in zip(sorted(links,key=lambda x:int(x['time_ps'])),(500000,up),(True,False),(False,True)):
   d=r['data'];assert r['time_ps']==d['scheduled_ps']==d['applied_ps']==str(time) and d['key']=={'link':'a-sw'} and d['before'] is before and d['after'] is after and d['outcome']=='changed'
  policy=group('policy');assert len(policy)==3 and [p['data']['topology_generation'] for p in policy]==['0','1','2']
  assert all(r['data']['kind'] in ('link_set','tree_publish','static_shadowed') for r in control)
  summary={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  for metric,v in {'generated':2,'serialized':len(tx),'received':2 if case=='on-time' else 1,'forwarded':2 if case=='on-time' else 1,'filtered':0 if case=='on-time' else 1,'dropped':0,'queued':0,'transmitting':0,'processing':0}.items():assert summary['$all','ethernet.'+metric]==str(v)
  if case=='late':assert summary['$all','ethernet.filtered.link_down']=='1'
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','compiler','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 values,prov=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-link-control-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==6 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-link-control',out/'zip');assert values==zipped
 report={'generated_by':'scripts/verify_guide_ethernet_link_control.py','status':'passed','source_tag':'v1.1.4','source_commit':COMMIT,'cli_operations':12,'zip_input_count':6,'single_variable':'controls[1].at_ps after common normalization','zip_reproduction':'all simulation objects identical','public_input_sha256':{p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCE.rglob('*') if p.is_file()},'conditions':values,'provenance':prov,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ethernet-link-control';dest.mkdir(parents=True,exist_ok=True);raw=(json.dumps(report,ensure_ascii=False,indent=2)+'\n').encode();(dest/'measurements.json').write_bytes(raw);(out/'measurements.json').write_bytes(raw)
 print('PASS: 12 formal CLI operations, exact one-ps variable, full timing/CRC/parent paths, control phase boundary, isolated ZIP equality')
if __name__=='__main__':main()
