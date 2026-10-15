"""Measure MAC aging as the only comparison variable using formal CLI and ZIP."""
import argparse,copy,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ethernet-learning';COMMIT='b45515644dfc65c205216d564d5bf7ef00280622';CASES={'short':2000000,'long':10000000}

def check_inputs():
 base=json.loads((SOURCE/'short.json').read_text());assert base['schema_version']==4 and base['switches'][0]['fdb']==[]
 prior=json.loads((ROOT/'examples/guide/ethernet-vlan/all.json').read_text());common=copy.deepcopy(base);common.pop('dynamic');common['schema_version']=prior['schema_version'];common['switches'][0]['fdb']=prior['switches'][0]['fdb'];assert common==prior
 for case,age in CASES.items():
  m=json.loads((SOURCE/(case+'.json')).read_text());assert m['dynamic']['mac_age_ps']==str(age);m['dynamic']['mac_age_ps']='2000000';assert m==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','MODEL.json')==(SOURCE/'short.ini').read_text().replace('short.json','MODEL.json')
 assert (SOURCE/'models/ethdemo/Main.ned').read_bytes()==(ROOT/'examples/ethernet/models/ethdemo/Main.ned').read_bytes()
 w=json.loads((SOURCE/'workload.json').read_text());assert w['schema_version']==4 and w['controls']==[] and len(w['generators'])==2
 for g,node,dst,time in zip(w['generators'],('Main.a','Main.b'),('02:00:00:00:00:02','02:00:00:00:00:01'),('0','5000000')):assert g['node']==node and g['times_ps']==[time] and g['frame']['dst_mac']==dst and g['priority']==0 and g['frame']['tag']=={'vid':10,'pcp':0,'dei':0}

def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case,age in CASES.items():
  result=out/case
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr
   if cmd[0]=='validate':assert json.loads(p.stdout)=={'channel_count':'6','network':'ethdemo.Main','node_count':'4','schema_version':1,'status':'valid'}
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']==COMMIT and str(meta['git_dirty'])=='false' and meta['binary_sha256']==hashlib.sha256(binary.read_bytes()).hexdigest()
  assert sim['end_ps']=='20000000' and sim['last_event_time_ps']==str(5609000+age) and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  group=lambda n:[r for r in sim['model_records'] if r['schema_name']=='ethernet.dynamic.'+n]
  assert len(group('frame'))==2;transfers=group('transfer');receptions=group('reception');assert len(transfers)==len(receptions)==(6 if case=='short' else 5)
  expected=[('outbound:0','Main.a.tx','Main.sw.rx_a',0,None),('outbound:0','Main.sw.tx_b','Main.b.rx',611000,'outbound:0/v0/c0'),('outbound:0','Main.sw.tx_c','Main.c.rx',611000,'outbound:0/v0/c0'),('return:0','Main.b.tx','Main.sw.rx_b',5000000,None),('return:0','Main.sw.tx_a','Main.a.rx',5611000,'return:0/v0/c3')]
  if case=='short':expected.append(('return:0','Main.sw.tx_c','Main.c.rx',5611000,'return:0/v0/c3'))
  for frame,port,peer,sof,parent in expected:
   t=next(t for t in transfers if t['data']['frame_id']==frame and t['data']['from_port']==port);d=t['data'];assert d['to_port']==peer and d['parent_transfer_id']==parent and d['status']=='serialized' and d['drop_reason'] is None
   assert d['sof_ps']==str(sof) and d['eof_ps']==str(sof+608000) and d['release_ps']==str(sof+704000) and d['arrival_ps']==str(sof+609000)
   wire=d['wire'];raw=bytes.fromhex(wire['mac_hex']);assert len(raw)==68 and wire['mac_bytes']=='68' and wire['tag']=={'vid':'10','pcp':'0','dei':'0'} and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little')
   rx=next(r['data'] for r in receptions if r['data']['transfer_id']==t['record_id']);assert rx['observed_ps']==d['arrival_ps']
   if peer.startswith('Main.sw.'):
    assert rx['status']=='forwarded' and rx['ready_ps']==str(sof+611000) and len(rx['egress_transfer_ids'])==(2 if frame=='outbound:0' or case=='short' else 1)
   else:assert rx['status']==('filtered' if peer=='Main.c.rx' else 'received') and rx['reason']==('destination_mismatch' if peer=='Main.c.rx' else None)
  controls=sorted(group('control'),key=lambda r:(int(r['time_ps']),int(r['data']['effect_seq'])));assert len(controls)==4
  wanted=sorted([(609000,'mac_learn','01','Main.sw.tx_a'),(609000+age,'mac_expire','01',None),(5609000,'mac_learn','02','Main.sw.tx_b'),(5609000+age,'mac_expire','02',None)])
  actual=[]
  for r in controls:
   d=r['data'];assert d['key']['switch']=='Main.sw' and d['key']['vid']=='10' and d['outcome']=='changed' and d['scheduled_ps']==d['applied_ps']==r['time_ps']
   if d['after']:assert d['after']['expires_at']==str(int(r['time_ps'])+age)
   actual.append((int(r['time_ps']),d['kind'],d['key']['mac'][-2:],d['after']['value'] if d['after'] else None))
  assert actual==wanted
  policies=group('policy');assert len(policies)==5 and all(r['data']['topology_generation']=='0' for r in policies)
  s={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  for metric,v in {'generated':2,'received':2,'serialized':len(transfers),'filtered':2 if case=='short' else 1,'forwarded':2,'dropped':0,'queued':0,'transmitting':0,'processing':0}.items():assert s['$all','ethernet.'+metric]==str(v)
  assert s['$all','ethernet.filtered.destination_mismatch']==('2' if case=='short' else '1')
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','compiler','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 public,prov=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-learning-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==6 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-learning',out/'zip');assert public==zipped
 d={'generated_by':'scripts/verify_guide_ethernet_learning.py','status':'passed','source_tag':'v1.1.4','source_commit':COMMIT,'cli_operations':12,'zip_input_count':6,'single_variable':'dynamic.mac_age_ps after common normalization','zip_reproduction':'all simulation objects identical','public_input_sha256':{p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCE.rglob('*') if p.is_file()},'conditions':public,'provenance':prov,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ethernet-learning';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(d,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS: 12 CLI operations; MAC learn/expire and half-open ages, exact copy paths/timing/CRC, reasons and isolated ZIP')
if __name__=='__main__':main()
