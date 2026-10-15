"""Measure a single CBS idle-slope parameter using formal CLI and isolated ZIP."""
import argparse,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
SOURCE=ROOT/'examples/guide/ethernet-cbs'
COMMIT='b45515644dfc65c205216d564d5bf7ef00280622'
RATES=(250000000,500000000,750000000)
CASES=tuple('slope'+str(r//1000000) for r in RATES)

def check_inputs():
 baseline=json.loads((ROOT/'examples/ethernet/tsn/cbs-model.json').read_text())
 assert json.loads((SOURCE/'slope250.json').read_text())==baseline
 baseline['tsn']['outputs'][0]['cbs'][0]['idle_slope_bps']='RATE'
 for case,rate in zip(CASES,RATES):
  model=json.loads((SOURCE/(case+'.json')).read_text());cbs=model['tsn']['outputs'][0]['cbs'][0]
  assert cbs['idle_slope_bps']==str(rate);cbs['idle_slope_bps']='RATE';assert model==baseline
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','MODEL.json')==(SOURCE/'slope250.ini').read_text().replace('slope250.json','MODEL.json')
 for local,original in [('models/ethtsn/Main.ned','models/ethtsn/Main.ned'),('workload.json','cbs-workload.json')]:
  assert (SOURCE/local).read_bytes()==(ROOT/'examples/ethernet/tsn'/original).read_bytes()

def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case,rate in zip(CASES,RATES):
  result=out/case;commands={}
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr;commands[cmd[0]]=json.loads(p.stdout)
  assert commands['validate']=={'channel_count':'2','network':'ethtsn.Net','node_count':'2','schema_version':1,'status':'valid'}
  assert commands['run']['exit_code']==0 and commands['run']['primary_diagnostic'] is None and commands['view']['status']=='complete'
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation']
  assert meta['git_commit']==COMMIT and str(meta['git_dirty'])=='false'
  # Independent specification arithmetic: occupied 672bit, R=1Gbps, no credit saturation.
  occupancy=672000;debt=(1000000000-rate)*occupancy
  recovery=(debt+rate-1)//rate;second=occupancy+recovery
  assert sim['end_ps']=='120000000' and sim['last_event_time_ps']==str(2*second) and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  rows=sim['model_records'];group=lambda name:[r for r in rows if r['schema_name']==name]
  frames=group('ethernet.dynamic.frame');transfers=group('ethernet.dynamic.transfer');receptions=group('ethernet.dynamic.reception')
  assert len(frames)==len(transfers)==len(receptions)==2
  for n,sof in enumerate([0,second]):
   t=next(t for t in transfers if t['record_id']==f'cbs:{n}/v0/c{n}');d=t['data']
   assert d['frame_id']==f'cbs:{n}' and d['from_port']=='Net.a.tx' and d['to_port']=='Net.b.rx' and d['queue_id']=='Net.a.tx.queue.7'
   assert d['parent_transfer_id'] is None and d['status']=='serialized' and d['drop_reason'] is None and d['queued_ps']=='0'
   assert d['sof_ps']==str(sof) and d['eof_ps']==d['arrival_ps']==str(sof+576000) and d['release_ps']==str(sof+occupancy)
   assert d['planned_eof_ps']==d['eof_ps'] and d['planned_arrival_ps']==d['arrival_ps'] and d['planned_release_ps']==d['release_ps']
   w=d['wire'];raw=bytes.fromhex(w['mac_hex']);assert w['mac_bytes']=='64' and w['pad_bytes']=='46' and w['data_hex']=='' and w['tag'] is None
   assert len(raw)==64 and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little')
   rx=next(r['data'] for r in receptions if r['data']['transfer_id']==t['record_id'])
   assert rx['status']=='received' and rx['reason'] is None and rx['observed_ps']==rx['ready_ps']==str(sof+576000)
  credit=group('ethernet.tsn.credit');assert len(credit)==7
  for release in (occupancy,second+occupancy):
   r=next(r for r in credit if r['time_ps']==str(release));d=r['data']
   assert d['port']=='Net.a.tx' and d['priority']=='7' and d['sign']=='negative' and d['magnitude']==str(debt) and d['scale']=='1000000000000' and d['slope_bps']==str(rate)
  assert any(r['time_ps']==str(second) and r['data']['magnitude']=='0' and r['data']['cause']=='sof' for r in credit)
  last=next(r for r in credit if r['time_ps']==str(2*second));assert last['data']['magnitude']=='0' and last['data']['slope_bps']=='0'
  waiting=[r for r in group('ethernet.tsn.decision') if r['data']['state']=='credit_negative'];assert waiting
  assert all(r['time_ps']==str(occupancy) and r['data']['next_wake_ps']==str(second) for r in waiting)
  assert all(r['data']['state'] not in ('guard_blocked','gate_closed','never_eligible') for r in group('ethernet.tsn.decision'))
  assert all(r['data']['verdict']=='bypass' for r in group('ethernet.tsn.policing'))
  summary={(r['target'],r['metric']):r['value'] for r in sim['summary']}
  for metric,v in {'generated':2,'serialized':2,'received':2,'dropped':0,'filtered':0,'queued':0,'transmitting':0,'processing':0}.items():assert summary['$all','ethernet.'+metric]==str(v)
  assert summary['Net.a.tx','ethernet.wire_bits']=='1152' and summary['Net.a.tx','ethernet.occupied_bits']=='1344'
  assert summary['@flow:cbs:Net.b','ethernet.flow.queue_wait_mean_ps']==second/2
  assert summary['@flow:cbs:Net.b','ethernet.flow.serialization_mean_ps']==576000
  assert summary['@flow:cbs:Net.b','ethernet.flow.delivery_mean_ps']==576000+second/2
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','compiler','binary_sha256','build_source_sha256','cargo_lock_sha256')}
  provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
 check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 public,provenance=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-cbs-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==8 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-cbs',out/'zip');assert public==zipped
 evidence={'generated_by':'scripts/verify_guide_ethernet_cbs.py','status':'passed','source_tag':'v1.1.4','source_commit':COMMIT,'cli_operations':18,'zip_input_count':8,'single_variable':'tsn.outputs[0].cbs[0].idle_slope_bps','zip_reproduction':'all simulation objects identical','public_input_sha256':{p.relative_to(ROOT).as_posix():hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCE.rglob('*') if p.is_file()},'conditions':public,'provenance':provenance,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ethernet-cbs';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS: 18 CLI operations; one idle slope, release-based credit/recovery/wake, FIFO timing, CRC and isolated ZIP')
if __name__=='__main__':main()
