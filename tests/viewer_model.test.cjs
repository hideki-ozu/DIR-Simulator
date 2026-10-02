'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const model = require('../crates/dir-simulator/viewer/model.js');

function fixture() {
  return {
    schema_version: 1, run_id: 'test-run',
    metadata: { initial_state: ['Main.a','Main.b','Main.c'].map(instance => ({instance,state:'{"queue":[]}'})).concat([{instance:'Main.bus',state:'{"profile":"can.cc.ideal.v1","state":"idle"}'}]) },
    simulation: {
      start_ps:'0',end_ps:'200',termination:'events_exhausted',partial:false,
      requests: [{ request_id:'g:0',source:'Main.a',bus:'Main.bus',status:'success',generated_ps:'10',ready_ps:'20',sof_ps:'30',eof_ps:'100',
        payload_bits:'8',serialized_bits:'52',model_fields:{profile:'can.cc.ideal.v1',schema_version:1,planned_eof_ps:'100',planned_release_ps:'110',release_ps:'110'}}],
      receivers: [
        {request_id:'g:0',receiver:'Main.b',status:'received',observed_ps:'120',received_ps:'130'},
        {request_id:'g:0',receiver:'Main.c',status:'filtered',observed_ps:'125',received_ps:null}
      ],
      records: [
        {seq:'0',time_ps:'0',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'0'},
        {seq:'1',time_ps:'20',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'1'},
        {seq:'2',time_ps:'30',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'0'}
      ],summary:[]
    }
  };
}
test('replay reflects reached milestones, queues, receiver delay and filters in both directions', () => {
  const m = model.parseResults(fixture());
  const cases = [[0n,'not_generated'],[10n,'processing'],[20n,'pending'],[30n,'in_flight'],[100n,'success'],[200n,'success']];
  for (const [t,state] of [...cases,...cases.toReversed()]) assert.equal(model.requestStateAt(m.requests[0],t),state);
  assert.equal(model.stateAt(m,20n).nodes[0].queue,1);
  assert.equal(model.stateAt(m,30n).nodes[0].queue,0);
  assert.equal(model.stateAt(m,99n).counts.rx_pending,0);
  assert.equal(model.stateAt(m,100n).counts.rx_pending,2);
  assert.equal(model.stateAt(m,100n).buses[0].state,'intermission');
  assert.equal(model.stateAt(m,110n).buses[0].state,'idle');
  assert.equal(model.stateAt(m,125n).counts.filtered,1);
  assert.equal(model.stateAt(m,129n).counts.received,0);
  assert.equal(model.stateAt(m,130n).counts.received,1);
  assert.equal(model.stateAt(m,200n).counts.generated,1);
});
test('EOF cutoff and unreached planned release remain transmitting/intermission', () => {
  const raw=fixture(),s=raw.simulation,r=s.requests[0];
  s.end_ps='100';s.termination='time_limit';r.status='in_flight';r.eof_ps=null;r.model_fields.release_ps=null;s.receivers=[];
  let m=model.parseResults(raw);
  assert.equal(model.stateAt(m,100n).buses[0].state,'transmitting');
  assert.equal(model.stateAt(m,100n).counts.success,0);
  assert.equal(m.events.some(e=>e.kind==='eof'),false);
  s.end_ps='105';r.status='success';r.eof_ps='100';
  m=model.parseResults(raw);
  assert.equal(model.stateAt(m,105n).buses[0].state,'intermission');
  assert.equal(m.events.some(e=>e.kind==='release'),false);
});
test('partial boundary keeps only committed milestones, including at H', () => {
  const raw=fixture(),s=raw.simulation;
  s.end_ps='100';s.termination='execution_failed';s.partial=true;
  s.requests[0].model_fields.release_ps=null;
  s.receivers.forEach(r=>{r.status='pending';r.observed_ps=null;r.received_ps=null;});
  const m=model.parseResults(raw);
  assert.equal(model.stateAt(m,100n).counts.success,1);
  assert.equal(model.stateAt(m,100n).counts.rx_pending,2);
  assert.equal(model.stateAt(m,99n).counts.success,0);
});
test('same-time queue measurements use record order, never reservation event_seq', () => {
  const raw=fixture();
  raw.simulation.records.push(
    {seq:'3',event_seq:'20',time_ps:'30',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'2'},
    {seq:'4',event_seq:'10',time_ps:'30',metric:'queue_length',target:'Main.a.txQueue',value_kind:'integer',value:'1'}
  );
  const m=model.parseResults(raw);
  assert.equal(model.stateAt(m,30n).nodes[0].queue,1);
  assert.equal(m.eventTimes.filter(t=>t===30n).length,1);
});
test('adjacent u64 times remain exact for cursor, formatting, order and state', () => {
  const raw=fixture(),s=raw.simulation,r=s.requests[0],base=9007199254740992n;
  s.end_ps=(base+100n).toString();s.records=[];s.receivers=[];
  r.generated_ps=base.toString();r.ready_ps=(base+1n).toString();r.sof_ps=(base+2n).toString();r.eof_ps=(base+3n).toString();
  r.model_fields.planned_eof_ps=r.eof_ps;r.model_fields.release_ps=(base+4n).toString();r.model_fields.planned_release_ps=r.model_fields.release_ps;
  const m=model.parseResults(raw);
  assert(m.eventTimes.includes(base+1n));assert(m.eventTimes.includes(base+2n));
  assert.equal(model.requestStateAt(m.requests[0],base+1n),'pending');
  assert.equal(model.requestStateAt(m.requests[0],base+2n),'in_flight');
  assert.equal(model.parseTime(model.formatTime(base+1n,'us'),'us'),base+1n);
  assert.equal(model.parseTime('18446744073709551615','ps'),(1n<<64n)-1n);
  assert.equal(model.timeFromFraction(base,base+2n,5000),base+1n);
});
test('idle nodes survive empty or zero-duration simulation via metadata', () => {
  const raw=fixture(),s=raw.simulation;
  s.end_ps='0';s.termination='time_limit';s.requests=[];s.receivers=[];s.records=[];
  const m=model.parseResults(raw);
  assert.deepEqual(m.nodes,['Main.a','Main.b','Main.c']);
  assert.deepEqual(m.eventTimes,[0n]);
  assert.equal(model.stateAt(m,0n).buses[0].state,'idle');
  assert.equal(model.fraction(0n,0n,0n),0);
  assert.equal(model.stateAt(m,0n).counts.generated,0);
});
test('drop transition occurs at ready, not generated; never generates success', () => {
  const raw=fixture(),s=raw.simulation,r=s.requests[0];
  s.receivers=[];r.status='dropped';r.sof_ps=null;r.eof_ps=null;
  r.model_fields.planned_eof_ps=null;r.model_fields.planned_release_ps=null;r.model_fields.release_ps=null;
  const m=model.parseResults(raw);
  assert.equal(model.requestStateAt(m.requests[0],19n),'processing');
  assert.equal(model.requestStateAt(m.requests[0],20n),'dropped');
  assert.equal(model.stateAt(m,200n).counts.success,0);
});
test('invalid schema, noncanonical time, statuses, references and chronology reject', () => {
  for(const change of [
    r=>r.schema_version=2,
    r=>r.simulation.end_ps=200,
    r=>r.simulation.end_ps='0200',
    r=>r.simulation.end_ps='18446744073709551616',
    r=>r.simulation.requests[0].generated_ps='21',
    r=>r.simulation.requests[0].status='pending',
    r=>r.simulation.requests[0].model_fields.profile='can.fd.v1',
    r=>r.simulation.receivers[0].request_id='absent',
    r=>r.simulation.receivers[0].observed_ps='1',
    r=>r.simulation.requests.push({...r.simulation.requests[0]}),
    r=>r.simulation.receivers.push({...r.simulation.receivers[0]}),
    r=>r.simulation.records[0].value='-1',
    r=>r.simulation.end_ps='130',
    r=>r.simulation.requests.push({...r.simulation.requests[0],request_id:'overlap'})
  ]) {const raw=fixture();change(raw);assert.throws(()=>model.parseResults(raw));}
});
test('time conversion rejects fractional ps and malformed numbers', () => {
  for(const value of ['-1','1e3','NaN','01','1.1']) assert.throws(()=>model.parseTime(value,'ps'));
  assert.throws(()=>model.parseTime('1','minutes'));
  assert.equal(model.parseTime('0.001','ns'),1n);
  assert.equal(model.formatTime(1001000n,'us'),'1.001');
  assert.equal(model.timeFromFraction(0n,100n,10000),100n);
});

test('network progress is exact, reversible and keeps delayed RX after bus release', () => {
  const raw=fixture();
  const next=structuredClone(raw.simulation.requests[0]);
  Object.assign(next,{request_id:'g:1',generated_ps:'110',ready_ps:'110',sof_ps:'110',eof_ps:'180'});
  Object.assign(next.model_fields,{planned_eof_ps:'180',release_ps:'190',planned_release_ps:'190'});
  raw.simulation.requests.push(next);
  const m=model.parseResults(raw),options={trailWindowPs:20n};
  assert.equal(model.networkAt(m,65n,options).tx[0].progress,.5);
  const at115=model.networkAt(m,115n,options);
  assert.equal(at115.tx[0].requestId,'g:1');
  assert.deepEqual(at115.rx.map(r=>r.phase),['observation','observation']);
  assert.equal(at115.rx[0].progress,.75);
  const at125=model.networkAt(m,125n,options);
  assert.equal(at125.rx.length,1);assert.equal(at125.rx[0].phase,'processing');
  assert.equal(at125.rx[0].progress,.5);
  assert.equal(at125.trails.find(r=>r.receiver==='Main.c').kind,'filtered');
  assert.equal(model.stateAt(m,125n).counts.success,1);
  assert.deepEqual(model.networkAt(m,115n,options),at115);
  assert.equal(model.networkAt(m,99n,options).rx.length,0);
  assert.equal(model.networkAt(m,99n,options).trails.length,0);
});
test('zero-delay broadcast completes simultaneously with deterministic visual trails', () => {
  const raw=fixture();
  raw.simulation.receivers.forEach(r=>Object.assign(r,{status:'received',observed_ps:'100',received_ps:'100'}));
  const m=model.parseResults(raw),options={trailWindowPs:20n};
  const before=model.networkAt(m,99n,options),at=model.networkAt(m,100n,options);
  assert.equal(before.tx.length,1);assert.equal(before.trails.length,0);
  assert.equal(at.tx.length,0);assert.equal(at.rx.length,0);
  assert.deepEqual(at.trails.filter(r=>r.kind==='received').map(r=>[r.receiver,r.time,r.opacity]),[['Main.b',100n,1],['Main.c',100n,1]]);
  assert(at.trails.every(r=>r.receiver!==r.source));
  assert(model.networkAt(m,110n,options).trails.every(r=>r.opacity===.5));
  assert.equal(model.networkAt(m,120n,options).trails.length,0);
  assert.equal(model.networkAt(m,100n,{trailWindowPs:0n}).trails.length,0);
  assert.equal(model.stateAt(m,100n).counts.received,2);
});
test('network EOF cutoff does not invent completion or receive records', () => {
  const raw=fixture(),s=raw.simulation,r=s.requests[0];
  s.end_ps='100';s.termination='time_limit';s.receivers=[];
  r.status='in_flight';r.eof_ps=null;r.model_fields.release_ps=null;
  const n=model.networkAt(model.parseResults(raw),100n);
  assert.equal(n.tx.length,1);assert.equal(n.tx[0].progress,1);
  assert.equal(n.buses[0].state,'transmitting');
  assert.equal(n.rx.length,0);assert.equal(n.trails.length,0);
  assert.equal(n.connections.find(c=>c.node==='Main.a').inferred,false);
  assert.equal(n.connections.find(c=>c.node==='Main.b').inferred,true);
});
test('active network packets are independent of the 500 timeline interval limit', () => {
  const raw=fixture(),s=raw.simulation,template=s.requests[0];
  s.requests=[];s.receivers=[];s.records=[];s.end_ps='200000';
  for(let i=0;i<600;i++){
    const r=structuredClone(template),base=BigInt(i)*200n;
    r.request_id=`g:${i}`;
    for(const field of ['generated_ps','ready_ps','sof_ps','eof_ps'])r[field]=(BigInt(r[field])+base).toString();
    for(const field of ['planned_eof_ps','planned_release_ps','release_ps'])r.model_fields[field]=(BigInt(r.model_fields[field])+base).toString();
    s.requests.push(r);
  }
  const n=model.networkAt(model.parseResults(raw),119865n);
  assert.equal(n.tx.length,1);assert.equal(n.tx[0].requestId,'g:599');
  assert.equal(n.tx[0].progress,.5);
  assert.equal(n.trails.length,1,'recent TX traces aggregate per sender');
});
test('network progress preserves precision for adjacent large timestamps', () => {
  const raw=fixture(),s=raw.simulation,r=s.requests[0],base=9007199254740992n;
  s.end_ps=(base+10n).toString();s.records=[];s.receivers=[];
  r.generated_ps=base.toString();r.ready_ps=base.toString();r.sof_ps=base.toString();r.eof_ps=(base+2n).toString();
  Object.assign(r.model_fields,{planned_eof_ps:r.eof_ps,release_ps:(base+3n).toString(),planned_release_ps:(base+3n).toString()});
  const m=model.parseResults(raw);
  assert.equal(model.networkAt(m,base+1n).tx[0].progress,.5);
  assert.equal(model.networkAt(m,base+2n).tx.length,0);
});
