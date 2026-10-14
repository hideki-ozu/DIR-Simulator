"""Measure static VLAN admission with an explicit formal CLI and isolated ZIP."""
import argparse,hashlib,json,subprocess,zipfile,zlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1];SOURCE=ROOT/'examples/guide/ethernet-vlan';CASES=('all','tagged','untagged');ADMIT=('all','tagged_only','untagged_only')
def check_inputs():
 base=json.loads((SOURCE/'all.json').read_text());assert base['ports'][3]['port']=='Main.sw.tx_a';base['ports'][3].pop('admit')
 for case,admit in zip(CASES,ADMIT):
  model=json.loads((SOURCE/(case+'.json')).read_text());assert model['ports'][3].pop('admit')==admit and model==base
  assert (SOURCE/(case+'.ini')).read_text().replace(case+'.json','MODEL.json')==(SOURCE/'all.ini').read_text().replace('all.json','MODEL.json')
def measure(binary,source,out):
 out.mkdir(parents=True);values={};provenance={}
 for case in CASES:
  rejected=case=='untagged';result=out/case;commands={}
  for cmd in [['validate','--config',str(source/(case+'.ini'))],['run','--config',str(source/(case+'.ini')),'--output',str(result)],['view','--input',str(result/'results.json'),'--output',str(out/(case+'-viewer.html'))]]:
   p=subprocess.run([str(binary),*cmd],capture_output=True,text=True);assert p.returncode==0,p.stderr;commands[cmd[0]]=json.loads(p.stdout)
  assert commands['validate']=={'channel_count':'6','network':'ethdemo.Main','node_count':'4','schema_version':1,'status':'valid'}
  assert commands['run']['exit_code']==0 and commands['run']['primary_diagnostic'] is None and commands['view']['status']=='complete'
  manifest=json.loads((result/'manifest.json').read_text());assert manifest['status']=='complete' and not manifest['partial']
  for f in manifest['files']:
   raw=(result/f['name']).read_bytes();assert str(len(raw))==f['bytes'] and hashlib.sha256(raw).hexdigest()==f['sha256']
  doc=json.loads((result/'results.json').read_text());meta,sim=doc['metadata'],doc['simulation'];assert meta['git_commit']=='b45515644dfc65c205216d564d5bf7ef00280622' and meta['git_dirty'] in (False,'false','clean')
  assert sim['end_ps']=='2000000' and sim['last_event_time_ps']==('704000' if rejected else '1315000') and sim['termination']=='events_exhausted' and not sim['partial'] and sim['pending_events']=='0'
  records=sim['model_records'];frames=[r for r in records if r['schema_name']=='ethernet.frame'];transfers=[r for r in records if r['schema_name']=='ethernet.transfer'];receptions=[r for r in records if r['schema_name']=='ethernet.reception'];assert len(frames)==1 and len(transfers)==(1 if rejected else 2) and len(receptions)==(1 if rejected else 2)
  f=frames[0];assert f['schema_version']==3 and f['record_id']=='probe:0';assert f['data']['source_vlan_id']=='10' and f['data']['tag']=={'vid':'10','pcp':'0','dei':'0'} and f['data']['mac_bytes']=='68' and f['data']['pad_bytes']=='46'
  for t in transfers:
   d=t['data'];wire=d['wire'];raw=bytes.fromhex(wire['mac_hex']);assert len(raw)==68 and raw[12:16]==bytes.fromhex('8100000a') and raw[-4:]==zlib.crc32(raw[:-4]).to_bytes(4,'little') and wire['fcs_hex']=='e4c9eb3a'
   start=0 if d['from_port']=='Main.a.tx' else 611000;assert t['schema_version']==3 and d['sof_ps']==str(start) and d['eof_ps']==d['planned_eof_ps']==str(start+608000) and d['release_ps']==d['planned_release_ps']==str(start+704000) and d['arrival_ps']==d['planned_arrival_ps']==str(start+609000) and d['drop_reason'] is None and d['status']=='serialized' and d['vlan_id']=='10'
  sw=next(r for r in receptions if r['subject']=='Main.sw')['data'];assert sw['observed_ps']=='609000' and sw['vlan_id']=='10' and sw['priority']=='0'
  if rejected:assert sw['ready_ps']=='609000' and sw['planned_ready_ps'] is None and sw['status']=='filtered' and sw['reason']=='ingress_frame_type' and sw['egress_transfer_ids']==[]
  else:
   assert sw['ready_ps']==sw['planned_ready_ps']=='611000' and sw['status']=='forwarded' and sw['reason'] is None and sw['egress_transfer_ids']==['probe:0@Main.sw.tx_b']
   rx=next(r for r in receptions if r['subject']=='Main.b')['data'];assert rx['observed_ps']==rx['ready_ps']==rx['planned_ready_ps']=='1220000' and rx['status']=='received' and rx['reason'] is None
  s={(r['target'],r['metric']):r['value'] for r in sim['summary']};assert s['$all','ethernet.generated']=='1' and s['$all','ethernet.received']==s['$all','ethernet.forwarded']==('0' if rejected else '1') and s['$all','ethernet.serialized']==('1' if rejected else '2')
  assert s['$all','ethernet.filtered']==s['$all','ethernet.filtered.ingress_frame_type']==('1' if rejected else '0') and s['$all','ethernet.dropped']=='0'
  reasons=[v for (target,metric),v in s.items() if target=='$all' and metric.startswith('ethernet.filtered.')];assert sum(map(int,reasons))==int(s['$all','ethernet.filtered'])
  for metric in ['ethernet.processing','ethernet.queued','ethernet.transmitting']:assert s['$all',metric]=='0'
  assert s['Main.a.tx','ethernet.mac_bits']=='544' and s['Main.a.tx','ethernet.wire_bits']=='608' and s['Main.a.tx','ethernet.occupied_bits']=='704' and s['Main.a.tx','ethernet.link_utilization']==.352
  target='@flow:flow_probe';assert s[target,'ethernet.flow.generated']=='1' and s[target,'ethernet.flow.unfinished_copies']==s[target,'ethernet.flow.copy_dropped']=='0' and s[target,'ethernet.flow.deadline_sample_count']=='0' and s[target,'ethernet.flow.deadline_miss_ratio'] is None
  if rejected:assert s[target,'ethernet.flow.delivery_mean_ps'] is None and s[target,'ethernet.flow.filtered']=='1'
  else:assert s[target,'ethernet.flow.serialization_mean_ps']==1216000 and s[target,'ethernet.flow.propagation_mean_ps']==2000 and s[target,'ethernet.flow.processing_mean_ps']==2000 and s[target,'ethernet.flow.queue_wait_mean_ps']==0 and s[target,'ethernet.flow.delivery_mean_ps']==1220000
  values[case]=sim;provenance[case]={k:meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')};provenance[case]['input_sha256']={s['logical_path']:s['sha256'] for s in meta['sources']}
 return values,provenance
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();check_inputs();out=a.output.resolve();out.mkdir(parents=True,exist_ok=False)
 public,provenance=measure(a.binary.resolve(),SOURCE,out/'repository')
 with zipfile.ZipFile(ROOT/'docs/guide/downloads/ethernet-vlan-inputs.zip') as z:
  expected={p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()};assert len(expected)==8 and set(z.namelist())==set(expected)
  for name,raw in expected.items():assert z.read(name)==raw
  z.extractall(out/'isolated')
 zipped,zprov=measure(a.binary.resolve(),out/'isolated/examples/guide/ethernet-vlan',out/'zip');assert public==zipped
 evidence={'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':8,'single_variable':'ports[3].admit','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':provenance,'zip_provenance':zprov}
 dest=ROOT/'docs/verification/results/ethernet-vlan';dest.mkdir(parents=True,exist_ok=True);raw=json.dumps(evidence,ensure_ascii=False,indent=2)+'\n';(dest/'measurements.json').write_text(raw);(out/'measurements.json').write_text(raw)
 print('PASS:18 CLI operations; strict one variable, CRC/tag/wire, arrival admission, reason sum, latency and isolated ZIP')
if __name__=='__main__':main()
