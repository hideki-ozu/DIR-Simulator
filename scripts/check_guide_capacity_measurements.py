"""Independent numeric/state checks for guide #116--#119 saved raw outputs."""
import argparse,csv,json,hashlib
from collections import Counter
from decimal import Decimal
from fractions import Fraction
from pathlib import Path

def load(p):return json.loads(p.read_text())
def check_summary_csv(csv_path,results_path):
 # Read the original JSON tokens: normal float loading loses precision before
 # comparison, even if the resulting float is later converted to Decimal.
 summary=json.loads(results_path.read_text(),parse_float=Decimal)['simulation']['summary']
 with csv_path.open() as source:csvrows=list(csv.DictReader(source))
 assert len(csvrows)==len(summary)
 for saved,r in zip(csvrows,summary):
  for key in ('metric','target','sample_count','start_ps','end_ps'):assert saved[key]==('' if r[key] is None else str(r[key]))
  assert (saved['value']=='' if r['value'] is None else saved['value']!='' and Decimal(saved['value'])==Decimal(r['value']))

def check(slug,folder):
 report=load(folder/'measurements.json');summaries={}
 for name,c in report['conditions'].items():
  sim=c.get('simulation') or load(folder/'repository'/name/'result/results.json')['simulation'];groups={}
  for r in sim['model_records']:groups.setdefault(r['schema_name'],[]).append(r)
  def rows(schema):return groups.get(schema,[])
  def ds(schema):return {r['record_id']:r['data'] for r in rows(schema)}
  def metric(target,metric):
   values=[r for r in sim['summary'] if r['target']==target and r['metric']==metric];assert len(values)==1,(target,metric);return values[0]
  assert not sim['partial']
  check_summary_csv(folder/'repository'/name/'result/summary.csv',folder/'repository'/name/'result/results.json')
  if slug=='ethernet-beb-seed':
   seed=int(name[-1]);attempts=ds('ethernet.attempt');transfers=ds('ethernet.transfer');receptions=ds('ethernet.reception')
   expect={1:([0,1],[2020000,8840000],[7880000,14700000],2),0:([1,1],[22400000,8100000],[28260000,13960000],3)}[seed]
   for i,x in enumerate('ab'):
    aid=f'{x}:0@Main.{x}.tx';first=attempts[aid+'#1']
    assert [int(first[k]) for k in ('sof_ps','collision_ps','jam_end_ps','backoff_slots')]==[0,100000,960000,expect[0][i]]
    assert first['eof_ps'] is None and first['arrival_ps'] is None
    t=transfers[aid];assert int(t['attempt_count'])==expect[3] and int(t['collision_count'])==expect[3]-1
    assert int(t['sof_ps'])==expect[1][i] and int(t['arrival_ps'])==expect[2][i]
    assert receptions[aid+'@rx']['ready_ps']==t['arrival_ps']
   for x in 'ab':
    assert int(metric(f'Main.{x}.tx','ethernet.media.jam_ps')['value'])==320000*(expect[3]-1)
    assert abs(float(metric(f'Main.{x}.tx','ethernet.media.tx_utilization')['value'])-(5760000+960000*(expect[3]-1))/100000000)<1e-12
   if seed==0:
    for i,x in enumerate('ab'):
     second=attempts[f'{x}:0@Main.{x}.tx#2'];assert second['sof_ps']=='6080000' and int(second['backoff_slots'])==[3,0][i]
   for key,val in [('attempts',2*expect[3]),('collisions',2*(expect[3]-1)),('delivered',2),('retry_exhausted',0)]:assert int(metric('$all','ethernet.media.'+key)['value'])==val
   summaries[name]={'attempts':attempts,'transfers':transfers,'receptions':receptions,'aggregate':{k:metric('$all','ethernet.media.'+k)['value'] for k in ['attempts','collisions','delivered','retry_exhausted']},'same_seed_repeat_logical_equal':c['same_seed_repeat_logical_equal']}
  elif slug=='axi-outstanding':
   cap=int(name[-1]);tx=ds('axi.transaction');a,b=tx['a:0'],tx['a:1'];assert (a['grant_ps'],a['completed_ps'],a['read_data'])==('0','20000',['00010203'])
   if cap==1:assert b['status']=='dropped' and b['drop_reason']=='outstanding_full' and b['grant_ps'] is None and b['completed_ps'] is None
   else:assert (b['grant_ps'],b['completed_ps'],b['read_data'])==('20000','40000',['00010203'])
   counts={k:int(metric('Main.m0','axi.'+k)['value']) for k in ['generated','completed','dropped','pending','active']};assert counts==dict(generated=2,completed=cap,dropped=2-cap,pending=0,active=0)
   assert counts['generated']==sum(counts[k] for k in ['completed','dropped','pending','active'])
   intervals=sorted((int(d['grant_ps']),int(d['completed_ps'])) for d in tx.values() if d['grant_ps'] is not None)
   assert all(b<=c for (a,b),(c,d) in zip(intervals,intervals[1:]))
   for key,expected in [('wait_mean_ps',0 if cap==1 else 10000),('latency_mean_ps',20000 if cap==1 else 30000)]:
    row=metric('Main.m0','axi.'+key);assert row['sample_count']==str(cap) and row['value']==expected
   summaries[name]={'transactions':tx,'handshakes':rows('axi.handshake'),'counts':counts,'manager_metrics':{r['metric']:r for r in sim['summary'] if r['target']=='Main.m0'},'bus_metrics':{r['metric']:r for r in sim['summary'] if r['target']=='Main.bus'}}
  elif slug=='ethernet-cbs-gate':
   closed=name=='closed';tx=ds('ethernet.dynamic.transfer');second=tx['cbs_gated:1/v0/c1'];expected=3688000 if closed else 2688000
   # Independent exact integration: negative slope includes 576ns MAC+preamble and96ns IFG.
   debit=Fraction(750000000*672000,10**12);assert debit==504
   recovered=Fraction(250000000*(1000000-672000),10**12);assert recovered==82
   remaining=(debit-recovered)*10**12//250000000;assert remaining==1688000
   assert expected==(2000000 if closed else 1000000)+remaining
   assert int(second['sof_ps'])==expected and int(second['eof_ps'])==expected+576000 and int(second['release_ps'])==expected+672000
   credit=rows('ethernet.tsn.credit')
   def bits(r):return Fraction(int(r['data']['magnitude']),int(r['data']['scale']))*(-1 if r['data']['sign']=='negative' else 1)
   assert any(r['time_ps']=='672000' and bits(r)==-504 for r in credit)
   if closed:
    for t in ['1000000','2000000']:assert any(r['time_ps']==t and bits(r)==-422 for r in credit)
   assert len(rows('ethernet.dynamic.reception'))==2
   summaries[name]={'transfers':tx,'credit':credit,'gates':rows('ethernet.tsn.gate'),'decisions':rows('ethernet.tsn.decision'),'independent_debit_bits':str(debit),'independent_remaining_ps':str(remaining)}
  else:
   cap=int(name[-1]);rx=ds('gw.rx_buffer');tx=ds('can.request');forwards=rows('gw.forward');receive=rows('can.receiver');counts=Counter((r['subject'],r['data']['status']) for r in rows('can.request'))
   assert len(rx)==10 and counts['Main.src','success']==10
   assert rx['rx:source:2/Main.gw/Main.gw.a']['released_ps']=='524000000'
   t2b=tx['gw:source:2/Main.gw/fanout/Main.gw.b'];t2c=tx['gw:source:2/Main.gw/fanout/Main.gw.c']
   assert t2b['model_fields']['tx_enqueued_ps']=='524000000' and t2b['sof_ps']=='948000000' and t2b['eof_ps']=='1348000000'
   assert t2c['eof_ps']=='370000000'
   parent3=rx['rx:source:3/Main.gw/Main.gw.a'];children3={k:v for k,v in tx.items() if v['model_fields'].get('parent_request_id')=='source:3'}
   assert parent3['status']==('dropped' if cap==1 else 'released') and len(children3)==(0 if cap==1 else 2)
   if cap==2:assert tx['gw:source:3/Main.gw/fanout/Main.gw.c']['eof_ps']=='480000000'
   rxcounts=Counter(d['status'] for d in rx.values());assert sum(rxcounts.values())==10 and rxcounts['released']==4+cap and rxcounts['dropped']==6-cap
   assert len(forwards)==2*(4+cap) and len(tx)-10==len(forwards)
   assert sum(counts[s,'success']+counts[s,'dropped']+counts[s,'processing']+counts[s,'waiting_tx']+counts[s,'pending']+counts[s,'in_flight'] for s in ['Main.gw.b','Main.gw.c'])==len(forwards)
   assert sim['termination']=='time_limit' and sim['pending_events']=='2' and not sim['partial']
   summaries[name]={'rx_buffers':rx,'requests':tx,'rx_counts':dict(rxcounts),'request_counts':{s+':'+st:n for (s,st),n in counts.items()},'receiver_counts':dict(Counter(r['subject'] for r in receive)),'source2_b_tx_buffer_wait_ps':str(int(t2b['model_fields']['tx_enqueued_ps'])-int(t2b['ready_ps'])),'source2_b_arbitration_wait_ps':str(int(t2b['sof_ps'])-int(t2b['model_fields']['tx_enqueued_ps'])),'termination':sim['termination'],'pending_events':sim['pending_events'],'partial':sim['partial']}
 report['status']='passed';report['independent_checks']=summaries
 for kind in ['conditions','zip_conditions']:
  for name,c in report[kind].items():
   c.pop('simulation',None)
   c['metadata']={k:v for k,v in c['metadata'].items() if k in ['git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256','compiler','toolchain','target_triple','os','cpu','started_at_utc','finished_at_utc','input_sha256','seed','runtime_version','config_sha256']}
   for operation,command in zip(['validate','run','view'],c['commands']):
    command.pop('stdout',None);command['log_path']=('repository' if kind=='conditions' else 'zip')+'/'+name+'/'+operation+'.log'
   c['results_path']=('repository' if kind=='conditions' else 'zip')+'/'+name+'/result/results.json';c['viewer_path']=('repository' if kind=='conditions' else 'zip')+'/'+name+'/viewer.html'
 (folder/'measurements.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
 print(slug+': independent expectations, conservation, CSV equality PASS')
if __name__=='__main__':
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('root',type=Path);p.add_argument('--artifacts',action='store_true',help='verify portable artifact-ledger hashes after numeric checks');p.add_argument('--only',choices=['ethernet-beb-seed','axi-outstanding','ethernet-cbs-gate','gateway-rx-capacity']);a=p.parse_args()
 for slug in ([a.only] if a.only else ['ethernet-beb-seed','axi-outstanding','ethernet-cbs-gate','gateway-rx-capacity']):
  check(slug,a.root/slug)
  if a.artifacts:
   ledger=load(a.root/slug/'artifact-ledger.json');root=Path(__file__).resolve().parents[1]
   for name,expected in ledger['files'].items():
    p=(root/name).resolve();p.relative_to(root);assert p.stat().st_size==expected['bytes'] and hashlib.sha256(p.read_bytes()).hexdigest()==expected['sha256'],name
   print(slug+': all portable artifacts match ledger PASS')
