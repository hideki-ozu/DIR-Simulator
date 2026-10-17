"""Verify one link-cost comparison with formal CLI, exact paths and isolated ZIP."""
import argparse,copy,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ethernet-stp-cost';COMMIT='b45515644dfc65c205216d564d5bf7ef00280622';CASES={'low':'10','high':'30'}
def load(p):return json.loads(p.read_text(encoding='utf-8'))
def expected_roles(case):
 r={p:'designated' for p in ('Net.a.tx','Net.b.tx','Net.c.tx','Net.s1.tx_a','Net.s1.tx_b','Net.s1.tx_c','Net.s2.tx_a','Net.s2.tx_b','Net.s2.tx_c','Net.s3.tx_a','Net.s3.tx_b','Net.s3.tx_c')};r['Net.s3.tx_b']='root';r.update({'Net.s2.tx_b':'root','Net.s3.tx_c':'alternate'} if case=='low' else {'Net.s2.tx_b':'alternate','Net.s2.tx_c':'root'});return r
def check_inputs():
 public=load(ROOT/'examples/ethernet/dynamic/model.json');base=copy.deepcopy(public)
 for sw in base['switches']:sw['fdb']=[]
 assert load(SOURCE/'low.json')==base
 for case,cost in CASES.items():
  m=load(SOURCE/(case+'.json'));link=next(x for x in m['dynamic']['links'] if x['id']=='L12');assert link['cost']==cost;link['cost']='10';assert m==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','MODEL.json')==(SOURCE/'low.ini').read_text().replace('low.json','MODEL.json')
 assert (SOURCE/'models/ethdynamic/Main.ned').read_bytes()==(ROOT/'examples/ethernet/dynamic/models/ethdynamic/Main.ned').read_bytes()
 assert (SOURCE/'workload.json').read_bytes()==(ROOT/'examples/ethernet/dynamic/unicast.json').read_bytes()
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case in CASES:
  result=out/case
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr
   if cmd[0]=='validate':assert json.loads(p.stdout)=={'channel_count':'12','network':'ethdynamic.Net','node_count':'6','schema_version':1,'status':'valid'}
  manifest=load(result/'manifest.json');assert manifest['status']=='complete' and manifest['partial'] is False
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=load(result/'results.json');meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']==COMMIT and str(meta['git_dirty'])=='false' and meta['binary_sha256']==hashlib.sha256(binary.read_bytes()).hexdigest()
  assert sim['end_ps']=='10000000' and sim['last_event_time_ps']==('6220000' if case=='low' else '6831000') and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  group=lambda n:[r for r in sim['model_records'] if r['schema_name']=='ethernet.dynamic.'+n]
  tx=group('transfer');rx=group('reception');assert len(group('frame'))==1 and len(tx)==len(rx)==6
  paths=[('Net.a.tx','Net.s1.rx_a',0,None,'forwarded',None),('Net.s1.tx_b','Net.s2.rx_b',611000,'Net.a.tx','forwarded' if case=='low' else 'filtered',None if case=='low' else 'stp_discarding'),('Net.s1.tx_c','Net.s3.rx_b',611000,'Net.a.tx','forwarded',None),('Net.s3.tx_a','Net.c.rx',1222000,'Net.s1.tx_c','filtered','destination_mismatch')]
  paths += [('Net.s2.tx_a','Net.b.rx',1222000,'Net.s1.tx_b','received',None),('Net.s2.tx_c','Net.s3.rx_c',1222000,'Net.s1.tx_b','filtered','stp_discarding')] if case=='low' else [('Net.s3.tx_c','Net.s2.rx_c',1222000,'Net.s1.tx_c','forwarded',None),('Net.s2.tx_a','Net.b.rx',1833000,'Net.s3.tx_c','received',None)]
  for port,peer,sof,parent,status,reason in paths:
   t=next(r for r in tx if r['data']['from_port']==port);d=t['data'];assert d['to_port']==peer and d['status']=='serialized' and d['drop_reason'] is None and d['frame_id']=='unicast:0';assert d['parent_transfer_id']==(None if parent is None else next(r['record_id'] for r in tx if r['data']['from_port']==parent));assert [d[x] for x in ('sof_ps','eof_ps','release_ps','arrival_ps')]==list(map(str,(sof,sof+608000,sof+704000,sof+609000)))
   raw=bytes.fromhex(d['wire']['mac_hex']);assert len(raw)==68 and d['wire']['tag']=={'vid':'10','pcp':'0','dei':'0'} and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little');reception=next(r['data'] for r in rx if r['data']['transfer_id']==t['record_id']);assert reception['observed_ps']==d['arrival_ps'] and reception['status']==status and reception['reason']==reason
   if status=='forwarded':assert reception['ready_ps']==str(sof+611000)
  policies=group('policy');assert all(p['data']['topology_generation']=='0' for p in policies);initial=next(p for p in policies if p['data']['initial']);state=initial['data']['changes'][0]['after'];assert state['roles']==expected_roles(case) and all(state['link_up'].values())
  learned={'Net.s1':(609000,'Net.s1.tx_a'),'Net.s3':(1220000,'Net.s3.tx_b'),'Net.s2':(1220000,'Net.s2.tx_b') if case=='low' else (1831000,'Net.s2.tx_c')};control=group('control');assert len(control)==6
  for sw,(arrival,port) in learned.items():
   learn=next(r for r in control if r['data']['kind']=='mac_learn' and r['data']['key']['switch']==sw);expire=next(r for r in control if r['data']['kind']=='mac_expire' and r['data']['key']['switch']==sw);assert learn['time_ps']==str(arrival) and learn['data']['after']['value']==port and learn['data']['after']['expires_at']==str(arrival+5000000);assert expire['time_ps']==str(arrival+5000000) and expire['data']['after'] is None
  s={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  for metric,v in {'generated':1,'serialized':6,'received':1,'forwarded':3,'filtered':2,'filtered.stp_discarding':1,'filtered.destination_mismatch':1,'dropped':0,'queued':0,'transmitting':0,'processing':0}.items():assert s['$all','ethernet.'+metric]==str(v)
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','compiler','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False);values,prov=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-stp-cost-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==6 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-stp-cost',out/'zip');assert values==zipped
 report={'generated_by':'scripts/verify_guide_ethernet_stp_cost.py','status':'passed','source_tag':'v1.1.4','source_commit':COMMIT,'cli_operations':12,'zip_input_count':6,'single_variable':'dynamic.links[L12].cost after common FDB clearing','zip_reproduction':'all simulation objects identical','public_input_sha256':{p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCE.rglob('*') if p.is_file()},'conditions':values,'provenance':prov,'zip_provenance':zprov};dest=ROOT/'docs/verification/results/ethernet-stp-cost';dest.mkdir(parents=True,exist_ok=True);raw=(json.dumps(report,ensure_ascii=False,indent=2)+'\n').encode();(dest/'measurements.json').write_bytes(raw);(out/'measurements.json').write_bytes(raw);print('PASS: 12 formal CLI operations; one cost variable, all roles/paths/times/CRC, blocked ingress and isolated ZIP equality')
if __name__=='__main__':main()
