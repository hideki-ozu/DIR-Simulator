"""Measure one logical Ethernet priority variable with formal CLI and isolated ZIP."""
import argparse,hashlib,json,subprocess,zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ethernet-priority';CASES=('priority0','priority3','priority7')
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case in CASES:
  priority=int(case[-1]);result=out/case;commands={}
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr;commands[cmd[0]]=json.loads(p.stdout)
  assert commands['validate']=={'channel_count':'6','network':'ethdemo.Main','node_count':'4','schema_version':1,'status':'valid'}
  assert commands['run']['exit_code']==0 and commands['run']['primary_diagnostic'] is None and commands['view']['status']=='complete'
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622' and meta['git_dirty'] in (False,'false','clean')
  assert sim['end_ps']=='3000000' and sim['last_event_time_ps']=='2595000' and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  frames={r['record_id']:r for r in sim['model_records'] if r['schema_name']=='ethernet.frame'};assert set(frames)=={'a_busy:0','b_probe:0','c_fixed:0'}
  order=['a_busy:0','c_fixed:0','b_probe:0'] if priority==0 else ['a_busy:0','b_probe:0','c_fixed:0']
  transfers={r['record_id']:r for r in sim['model_records'] if r['schema_name']=='ethernet.transfer'};assert len(transfers)==6
  receptions={r['record_id']:r for r in sim['model_records'] if r['schema_name']=='ethernet.reception'};assert len(receptions)==6
  for index,rid in enumerate(order):
   f=frames[rid];d=f['data'];generated=0 if rid=='a_busy:0' else 100000;pr=priority if rid=='b_probe:0' else (3 if rid=='c_fixed:0' else 0)
   assert f['schema_version']==2 and d['source']=='Main.a' and d['generated_ps']==d['ready_ps']==str(generated) and d['priority']==str(pr) and d['deadline_ps'] is None
   assert d['data_hex']=='' and d['mac_bytes']=='64' and d['pad_bytes']=='46' and d['src_mac']=='02:00:00:00:00:01' and d['dst_mac']=='02:00:00:00:00:02' and d['ether_type']=='2048' and d['fcs_hex']=='a9e82eb4'
   start=index*672000;received=1156000+index*672000
   for port,to,g,q,parent in [('Main.a.tx','Main.sw.rx_a',start,generated,None),('Main.sw.tx_b','Main.b.rx',start+579000,start+579000,rid+'@Main.a.tx')]:
    t=transfers[rid+'@'+port];x=t['data'];assert t['schema_version']==2 and t['subject']==port and t['request_id']==rid
    assert x['sof_ps']==str(g) and x['queued_ps']==str(q) and x['eof_ps']==x['planned_eof_ps']==str(g+576000) and x['release_ps']==x['planned_release_ps']==str(g+672000)
    assert x['arrival_ps']==x['planned_arrival_ps']==str(g+577000) and x['from_port']==port and x['to_port']==to and x['parent_transfer_id']==parent and x['status']=='serialized' and x['drop_reason'] is None
    assert x['priority']==str(pr) and x['queue_id']==port+'.queue.'+str(pr) and t['time_ps']==x['release_ps']
   sw=receptions[rid+'@Main.a.tx@rx'];rx=receptions[rid+'@Main.sw.tx_b@rx']
   assert sw['subject']=='Main.sw' and sw['data']['status']=='forwarded' and sw['data']['observed_ps']==str(start+577000) and sw['data']['ready_ps']==str(start+579000) and sw['data']['egress_transfer_ids']==[rid+'@Main.sw.tx_b']
   assert rx['subject']=='Main.b' and rx['data']['status']=='received' and rx['data']['observed_ps']==rx['data']['ready_ps']==rx['data']['planned_ready_ps']==str(received) and rx['data']['reason'] is None
  s={(r['target'],r['metric']):r['value'] for r in sim['summary']};assert s['$all','ethernet.generated']==s['$all','ethernet.received']==s['$all','ethernet.forwarded']=='3' and s['$all','ethernet.serialized']=='6'
  for metric in ['ethernet.dropped','ethernet.filtered','ethernet.processing','ethernet.queued','ethernet.transmitting']:assert s['$all',metric]=='0'
  for port in ['Main.a.tx','Main.sw.tx_b']:
   assert s[port,'ethernet.mac_bits']=='1536' and s[port,'ethernet.wire_bits']=='1728' and s[port,'ethernet.occupied_bits']=='2016' and s[port,'ethernet.payload_bits']=='0' and s[port,'ethernet.link_utilization']==.672
  assert s['Main.a.tx.queue','queue_max']=='2' and s['Main.a.tx.queue.3','queue_bytes_max']==('128' if priority==3 else '64')
  for flow,delay in [('flow_busy',1156000),('flow_probe',2400000 if priority==0 else 1728000),('flow_fixed',1728000 if priority==0 else 2400000)]:
   target='@flow:'+flow;assert s[target,'ethernet.flow.generated']==s[target,'ethernet.flow.received']=='1' and s[target,'ethernet.flow.unfinished_copies']==s[target,'ethernet.flow.copy_dropped']=='0'
   assert s[target,'ethernet.flow.delivery_mean_ps']==delay and s[target,'ethernet.flow.serialization_mean_ps']==1152000 and s[target,'ethernet.flow.propagation_mean_ps']==2000 and s[target,'ethernet.flow.processing_mean_ps']==2000
   assert s[target,'ethernet.flow.queue_wait_mean_ps']+1156000==delay and s[target,'ethernet.flow.deadline_sample_count']=='0' and s[target,'ethernet.flow.deadline_miss_ratio'] is None
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 base=json.loads((SOURCE/'priority0.json').read_text());base['generators'][1].pop('priority')
 for case in CASES:
  model=json.loads((SOURCE/(case+'.json')).read_text());assert model['generators'][1].pop('priority')==int(case[-1]) and model==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','WORKLOAD.json')==(SOURCE/'priority0.ini').read_text().replace('priority0.json','WORKLOAD.json')
 public,provenance=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-priority-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==8 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-priority',out/'zip');assert public==zipped
 evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':8,'single_variable':'generators[1].priority','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':provenance,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ethernet-priority';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS: 18 CLI operations; one logical priority, FIFO/nonpreemption, 3frames/6copies/receptions, delays/bits/hash and isolated ZIP')
if __name__=='__main__':main()
