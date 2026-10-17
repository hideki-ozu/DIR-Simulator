"""Verify VID registration lifetime as the only variable using formal CLI and isolated ZIP."""
import argparse,copy,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ethernet-vid-registration';COMMIT='b45515644dfc65c205216d564d5bf7ef00280622';CASES={'short':2000000,'long':5000000}
def load(p):return json.loads(p.read_text(encoding='utf-8'))
def check_inputs():
 base=load(SOURCE/'short.json');public=load(ROOT/'examples/ethernet/dynamic/registration.json');expected=copy.deepcopy(public);expected['generators'][0].update(node='Main.a',times_ps=['0','3000000']);expected['generators'][0]['frame']['tag']['vid']=20;template=public['controls'][0];expected['controls']=[dict(template,id=id,at_ps='0',port=port,lifetime_ps='2000000' if id=='egress' else '8000000') for id,port in [('source','Main.a.tx'),('ingress','Main.sw.tx_a'),('receiver','Main.b.tx'),('egress','Main.sw.tx_b')]];assert base==expected
 for case,life in CASES.items():
  w=load(SOURCE/(case+'.json'));assert w['controls'][3]['lifetime_ps']==str(life);w['controls'][3]['lifetime_ps']='2000000';assert w==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','WORKLOAD.json')==(SOURCE/'short.ini').read_text().replace('short.json','WORKLOAD.json')
 prior=load(ROOT/'examples/guide/ethernet-link-control/model.json');prior['switches'][0]['fdb'] += [dict(x,vid=20) for x in prior['switches'][0]['fdb']];prior['dynamic']['registrable']=[{'port':p['port'],'vid':20,'tagged':True} for p in prior['ports']];assert load(SOURCE/'model.json')==prior
 assert (SOURCE/'models/ethdemo/Main.ned').read_bytes()==(ROOT/'examples/ethernet/models/ethdemo/Main.ned').read_bytes()
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case,life in CASES.items():
  result=out/case
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr
   if cmd[0]=='validate':assert json.loads(p.stdout)=={'channel_count':'6','network':'ethdemo.Main','node_count':'4','schema_version':1,'status':'valid'}
  manifest=load(result/'manifest.json');assert manifest['status']=='complete' and manifest['partial'] is False
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=load(result/'results.json');meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']==COMMIT and str(meta['git_dirty'])=='false' and meta['binary_sha256']==hashlib.sha256(binary.read_bytes()).hexdigest()
  assert sim['end_ps']=='10000000' and sim['last_event_time_ps']=='8000000' and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  group=lambda n:[r for r in sim['model_records'] if r['schema_name']=='ethernet.dynamic.'+n]
  assert len(group('frame'))==2;tx=group('transfer');rx=group('reception');assert len(tx)==len(rx)==(3 if case=='short' else 4)
  expected=[('unicast:0','Main.a.tx','Main.sw.rx_a',0),('unicast:0','Main.sw.tx_b','Main.b.rx',611000),('unicast:1','Main.a.tx','Main.sw.rx_a',3000000)]
  if case=='long':expected.append(('unicast:1','Main.sw.tx_b','Main.b.rx',3611000))
  for frame,port,peer,sof in expected:
   t=next(r for r in tx if r['data']['frame_id']==frame and r['data']['from_port']==port);d=t['data'];assert d['to_port']==peer and d['status']=='serialized' and d['drop_reason'] is None
   assert [d[x] for x in ('sof_ps','eof_ps','release_ps','arrival_ps')]==list(map(str,(sof,sof+608000,sof+704000,sof+609000)))
   assert d['parent_transfer_id']==(None if port=='Main.a.tx' else next(r['record_id'] for r in tx if r['data']['frame_id']==frame and r['data']['from_port']=='Main.a.tx'))
   raw=bytes.fromhex(d['wire']['mac_hex']);assert len(raw)==68 and d['wire']['tag']=={'vid':'20','pcp':'0','dei':'0'} and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little')
   reception=next(r['data'] for r in rx if r['data']['transfer_id']==t['record_id']);assert reception['observed_ps']==d['arrival_ps'];filtered=case=='short' and frame=='unicast:1';assert reception['status']==('filtered' if filtered else 'forwarded' if peer=='Main.sw.rx_a' else 'received');assert reception['reason']==('known_egress_ineligible' if filtered else None)
   if peer=='Main.sw.rx_a':assert reception['ready_ps']==str(sof+611000) and len(reception['egress_transfer_ids'])==(0 if filtered else 1) and reception['policy_epoch_ingress']==('2' if filtered else '1')
  controls=group('control');assert len(controls)==10
  for port in ['Main.a.tx','Main.sw.tx_a','Main.b.tx','Main.sw.tx_b']:
   regs=[r for r in controls if r['data']['kind']=='vlan_register' and r['data']['key']['port']==port];exps=[r for r in controls if r['data']['kind']=='registration_expire' and r['data']['key']['port']==port];assert len(regs)==len(exps)==1;ttl=life if port=='Main.sw.tx_b' else 8000000
   for r,time in [(regs[0],0),(exps[0],ttl)]:assert r['time_ps']==r['data']['scheduled_ps']==r['data']['applied_ps']==str(time) and r['data']['key']['vid']=='20' and r['data']['outcome']=='changed'
   assert regs[0]['data']['after']==exps[0]['data']['before']=={'expires_at':str(ttl),'generation':'1','value':True} and exps[0]['data']['after'] is None
  policy=group('policy');assert len(policy)==4 and all(p['data']['topology_generation']=='0' for p in policy)
  s={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  for metric,v in {'generated':2,'serialized':len(tx),'received':1 if case=='short' else 2,'forwarded':1 if case=='short' else 2,'filtered':1 if case=='short' else 0,'dropped':0,'queued':0,'transmitting':0,'processing':0}.items():assert s['$all','ethernet.'+metric]==str(v)
  if case=='short':assert s['$all','ethernet.filtered.known_egress_ineligible']=='1'
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','compiler','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False);values,prov=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-vid-registration-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==6 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-vid-registration',out/'zip');assert values==zipped
 report={'generated_by':'scripts/verify_guide_ethernet_vid_registration.py','status':'passed','source_tag':'v1.1.4','source_commit':COMMIT,'cli_operations':12,'zip_input_count':6,'single_variable':'controls[3].lifetime_ps after common normalization','zip_reproduction':'all simulation objects identical','public_input_sha256':{p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCE.rglob('*') if p.is_file()},'conditions':values,'provenance':prov,'zip_provenance':zprov};dest=ROOT/'docs/verification/results/ethernet-vid-registration';dest.mkdir(parents=True,exist_ok=True);raw=(json.dumps(report,ensure_ascii=False,indent=2)+'\n').encode();(dest/'measurements.json').write_bytes(raw);(out/'measurements.json').write_bytes(raw);print('PASS: 12 formal CLI operations; one lifetime variable, all copy timing/CRC/parent paths, per-port registration/expiry and isolated ZIP equality')
if __name__=='__main__':main()
