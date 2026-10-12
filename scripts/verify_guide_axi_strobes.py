"""Measure AXI byte strobes with formal CLI and an isolated public ZIP."""
import argparse,hashlib,json,subprocess,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/axi-strobes';CASES=(0,5,15);BYTES={0:'00010203',5:'11013303',15:'11223344'}
def measure(binary,source,out):
 out.mkdir(parents=True);values={};prov={}
 for mask in CASES:
  name=f'strobe{mask}';result=out/name
  for command in [['validate','--config',str(source/(name+'.ini'))],['run','--config',str(source/(name+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(name+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*command],capture_output=True,text=True);assert p.returncode==0,p.stderr;reply=json.loads(p.stdout)
   assert reply.get('status') in ('valid','complete') or reply.get('exit_code')==0
   if command[0]=='validate':assert reply['node_count']=='3' and reply['channel_count']=='0' and reply['network']=='strobes.Main'
  man=json.loads((result/'manifest.json').read_text());assert man['status']=='complete' and not man['partial']
  for f in man['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,s=doc['metadata'],doc['simulation']
  assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622' and meta['git_dirty'] in (False,'false','clean')
  assert s['end_ps']=='100000' and s['last_event_time_ps']=='70000' and s['termination']=='events_exhausted' and s['pending_events']=='0' and not s['partial']
  rows={x['request_id']:x for x in s['model_records'] if x['schema_name']=='axi.transaction'};assert len(rows)==2
  for rid,g,end,operation in [('write:0','0','30000','write'),('read:0','50000','70000','read')]:
   d=rows[rid]['data'];assert d['generated_ps']==d['eligible_ps']==d['grant_ps']==g and d['completed_ps']==end
   assert d['operation']==operation and d['status']=='completed' and d['response']=='OKAY' and d['drop_reason'] is None and d['address']=='4096' and d['beats']=='1'
   assert d['manager']=='Main.m0' and d['interconnect']=='Main.bus' and d['target']=='Main.ram'
  assert rows['read:0']['data']['read_data']==[BYTES[mask]]
  hands={x['data']['channel']:x for x in s['model_records'] if x['schema_name']=='axi.handshake'};assert len(hands)==5
  for channel,time,valid in [('AW','10000','10000'),('W','20000','10000'),('B','30000','30000'),('AR','60000','60000'),('R','70000','70000')]:
   h=hands[channel];assert h['time_ps']==time and h['data']['valid_since_ps']==valid and h['subject']=='Main.bus'
  assert hands['W']['data']['data_hex']=='11223344' and hands['W']['data']['wstrb']==str(mask) and hands['W']['data']['last']
  assert hands['R']['data']['data_hex']==BYTES[mask] and hands['R']['data']['last'] and hands['R']['data']['response']=='OKAY' and hands['B']['data']['response']=='OKAY'
  memory=next(x['data'] for x in s['model_records'] if x['schema_name']=='axi.memory');assert memory=={'base':'4096','data_hex':BYTES[mask]+'04050607'+'00'*8,'size':'16'}
  summaries={(x['target'],x['metric']):x['value'] for x in s['summary']};assert summaries['Main.bus','axi.written_bits']==str(mask.bit_count()*8)
  assert summaries['Main.bus','axi.read_bits']=='32' and summaries['Main.bus','axi.bus_utilization']==.5
  for target in ['$all','Main.bus','Main.m0']:
   assert summaries[target,'axi.generated']==summaries[target,'axi.completed']==summaries[target,'axi.okay']=='2'
   assert summaries[target,'axi.dropped']==summaries[target,'axi.slverr']==summaries[target,'axi.decerr']=='0'
  values[name]=s;prov[name]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')};prov[name]['input_sha256']={x['logical_path']:x['sha256'] for x in meta['sources']}
 return values,prov
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 base=json.loads((SOURCE/'strobe0.json').read_text());base['generators'][0]['transaction'].pop('write_strobes')
 for mask in CASES:
  name=f'strobe{mask}';v=json.loads((SOURCE/(name+'.json')).read_text());assert v['generators'][0]['transaction'].pop('write_strobes')==[mask] and v==base
  assert (SOURCE/(name+'.ini')).read_text().replace(name+'.json','WORKLOAD.json')==(SOURCE/'strobe0.ini').read_text().replace('strobe0.json','WORKLOAD.json')
 public,prov=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/axi-strobes-inputs.zip') as z:
  expected={x.relative_to(ROOT).as_posix():x.read_bytes() for x in SOURCE.rglob('*') if x.is_file()};assert set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/axi-strobes',out/'zip');assert public==zipped
 evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':len(expected),'single_variable':'generators[0].transaction.write_strobes[0]','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':prov,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/axi-strobes';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS: 18 CLI operations; WSTRB byte updates, five handshakes, responses, metrics, manifest, isolated ZIP')
if __name__=='__main__':main()
