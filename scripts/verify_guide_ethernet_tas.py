"""Measure one TAS generation-time boundary with formal CLI and isolated ZIP."""
import argparse,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ethernet-tas';TIMES=(127999,128000,128001);CASES=tuple('time'+str(t) for t in TIMES)
def check_inputs():
 base=json.loads((SOURCE/'time127999.json').read_text());assert len(base['generators'])==1 and len(base['generators'][0]['times_ps'])==1;base['generators'][0]['times_ps'].pop()
 for case,time in zip(CASES,TIMES):
  workload=json.loads((SOURCE/(case+'.json')).read_text());assert len(workload['generators'][0]['times_ps'])==1 and workload['generators'][0]['times_ps'].pop()==str(time) and workload==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','WORKLOAD.json')==(SOURCE/'time127999.ini').read_text().replace('time127999.json','WORKLOAD.json')
 assert (SOURCE/'model.json').read_bytes()==(ROOT/'examples/ethernet/tsn/tas-model.json').read_bytes()
 assert (SOURCE/'models/ethtsn/Main.ned').read_bytes()==(ROOT/'examples/ethernet/tsn/models/ethtsn/Main.ned').read_bytes()
 model=json.loads((SOURCE/'model.json').read_text());assert model['tsn']['clock']=='ideal_shared' and model['tsn']['streams']==model['tsn']['gcl_updates']==[]
 tas=model['tsn']['outputs'][0];assert tas['port']=='Net.a.tx' and tas['cbs']==[] and tas['tas']=={'id':'g0','base_time_ps':'0','cycle_time_ps':'2000000','entries':[{'duration_ps':'800000','open_priorities':[7]},{'duration_ps':'1200000','open_priorities':[]}]}
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case,generated in zip(CASES,TIMES):
  late=generated==128001;sof=2000000 if late else generated;eof=sof+576000;release=sof+672000;result=out/case;commands={}
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr;commands[cmd[0]]=json.loads(p.stdout)
  assert commands['validate']=={'channel_count':'2','network':'ethtsn.Net','node_count':'2','schema_version':1,'status':'valid'}
  assert commands['run']['exit_code']==0 and commands['run']['primary_diagnostic'] is None and commands['view']['status']=='complete'
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622' and meta['git_dirty'] in (False,'false','clean')
  assert sim['end_ps']=='3000000' and sim['last_event_time_ps']==str(release) and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  rows=sim['model_records'];group=lambda name:[r for r in rows if r['schema_name']==name]
  frames=group('ethernet.dynamic.frame');transfers=group('ethernet.dynamic.transfer');receptions=group('ethernet.dynamic.reception');assert len(frames)==len(transfers)==len(receptions)==1
  f=frames[0];assert f['record_id']=='tas:0' and f['schema_version']==1 and f['data']['generated_ps']==f['data']['ready_ps']==str(generated) and f['data']['priority']=='7' and f['data']['tag'] is None and f['data']['deadline_ps'] is None
  t=transfers[0];d=t['data'];assert t['record_id']=='tas:0/v0/c0' and t['schema_version']==1 and d['from_port']=='Net.a.tx' and d['to_port']=='Net.b.rx' and d['queue_id']=='Net.a.tx.queue.7' and d['queued_ps']==str(generated) and d['sof_ps']==str(sof) and d['eof_ps']==d['planned_eof_ps']==str(eof) and d['release_ps']==d['planned_release_ps']==str(release) and d['arrival_ps']==d['planned_arrival_ps']==str(eof) and d['status']=='serialized' and d['parent_transfer_id'] is None and d['drop_reason'] is None
  wire=d['wire'];raw=bytes.fromhex(wire['mac_hex']);assert len(raw)==64 and wire['mac_bytes']=='64' and wire['pad_bytes']=='46' and wire['data_hex']=='' and wire['tag'] is None and wire['fcs_hex']=='a9e82eb4' and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little')
  rx=receptions[0]['data'];assert rx['observed_ps']==rx['ready_ps']==rx['planned_ready_ps']==str(eof) and rx['ingress']=='Net.b.rx' and rx['status']=='received' and rx['reason'] is None and rx['egress_transfer_ids']==[]
  gate=next(r for r in group('ethernet.tsn.gate') if r['data']['port']=='Net.a.tx');assert gate['time_ps']=='0' and gate['data']['schedule_id']=='g0' and gate['data']['open_priorities']==['7'] and gate['data']['next_boundary_ps']=='800000'
  blocked=[r for r in group('ethernet.tsn.decision') if r['data']['state']=='guard_blocked'];assert len(blocked)==(2 if late else 0)
  for b in blocked:assert b['time_ps']==str(generated) and b['data']['port']=='Net.a.tx' and b['data']['priority']=='7' and b['data']['next_wake_ps']=='2000000'
  assert group('ethernet.tsn.credit')==[];policing=group('ethernet.tsn.policing');assert len(policing)==1 and policing[0]['data']['verdict']=='bypass' and policing[0]['data']['stream_id'] is None and policing[0]['data']['consumed_bits']=='0'
  s={(r['target'],r['metric']):r['value'] for r in sim['summary']};assert s['$all','ethernet.generated']==s['$all','ethernet.received']==s['$all','ethernet.serialized']=='1'
  for metric in ['ethernet.dropped','ethernet.filtered','ethernet.processing','ethernet.queued','ethernet.transmitting','ethernet.forwarded']:assert s['$all',metric]=='0'
  assert s['Net.a.tx','ethernet.mac_bits']=='512' and s['Net.a.tx','ethernet.wire_bits']=='576' and s['Net.a.tx','ethernet.occupied_bits']=='672' and s['Net.a.tx','ethernet.link_utilization']==.224 and s.get(('Net.a.tx','ethernet.tsn.guard_wait'),'0')==('2' if late else '0')
  target='@flow:tas';assert s[target,'ethernet.flow.generated']==s[target,'ethernet.flow.received']=='1' and s[target,'ethernet.flow.copy_dropped']==s[target,'ethernet.flow.unfinished_copies']=='0'
  assert s[target,'ethernet.flow.serialization_mean_ps']==576000 and s[target,'ethernet.flow.propagation_mean_ps']==s[target,'ethernet.flow.processing_mean_ps']==0 and s[target,'ethernet.flow.queue_wait_mean_ps']==sof-generated and s[target,'ethernet.flow.delivery_mean_ps']==eof-generated and s[target,'ethernet.flow.deadline_sample_count']=='0' and s[target,'ethernet.flow.deadline_miss_ratio'] is None
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 public,provenance=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-tas-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==8 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-tas',out/'zip');assert public==zipped
 evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':8,'single_variable':'generators[0].times_ps[0] after common normalization to one frame','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':provenance,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ethernet-tas';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS:18 CLI operations; one generation-time, IFG-inclusive boundary/guard, wake vs gate boundary, timing/bits/CRC and isolated ZIP')
if __name__=='__main__':main()
