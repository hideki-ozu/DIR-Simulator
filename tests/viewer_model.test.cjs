'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const model = require('../crates/dir-simulator/src/tool/viewer/assets/model.js');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {execFileSync} = require('node:child_process');
const {fixture, rxHoldingFixture} = require('./viewer_fixtures.cjs');

test('request event groups retain exact ps and only reached request/receiver milestones', () => {
  const m=model.parseResults(fixture());
  assert.deepEqual(model.requestEventGroups(m,'g:0').map(g=>g.time),[10n,20n,30n,100n,110n,120n,125n,130n]);
  assert.deepEqual(model.requestEventGroups(m,'absent'),[]);
  const raw=fixture();
  raw.simulation.partial=true;raw.simulation.end_ps='40';
  Object.assign(raw.simulation.requests[0],{status:'in_flight',eof_ps:null});
  raw.simulation.requests[0].model_fields.release_ps=null;
  raw.simulation.receivers=[];
  assert.deepEqual(model.requestEventGroups(model.parseResults(raw),'g:0').map(g=>g.time),[10n,20n,30n]);
  const exact=fixture(),offset=(1n<<64n)-201n;
  for(const request of exact.simulation.requests) {
    for(const field of ['generated_ps','ready_ps','sof_ps','eof_ps']) request[field]=(offset+BigInt(request[field])).toString();
    for(const field of ['planned_eof_ps','planned_release_ps','release_ps']) request.model_fields[field]=(offset+BigInt(request.model_fields[field])).toString();
  }
  for(const receiver of exact.simulation.receivers)for(const field of ['observed_ps','received_ps'])if(receiver[field]!==null)receiver[field]=(offset+BigInt(receiver[field])).toString();
  exact.simulation.records=[];exact.simulation.end_ps=((1n<<64n)-1n).toString();
  assert.equal(model.requestEventGroups(model.parseResults(exact),'g:0')[0].time,offset+10n);
  const empty=fixture();empty.simulation.requests=[];empty.simulation.receivers=[];
  assert.deepEqual(model.requestEventGroups(model.parseResults(empty),'g:0'),[]);
});

test('request event groups group simultaneous events and keep GW fanout branches separate', () => {
  const raw=fixture();raw.simulation.requests[0].generated_ps='0';raw.simulation.requests[0].ready_ps='0';
  const m=model.parseResults(raw);
  m.requests.push({id:'copy-a'},{id:'copy-b'});
  m.events.push({time:130n,kind:'gw_rx_hold',requestId:'g:0',node:'gw.a'},
    {time:140n,kind:'gw_rx_release',requestId:'g:0',node:'gw.a'},
    {time:130n,kind:'gw_received',requestId:'g:0',node:'gw'},
    {time:140n,kind:'generated',requestId:'copy-a',node:'gw.b'},
    {time:140n,kind:'generated',requestId:'copy-b',node:'gw.c'});
  m.events.sort((a,b)=>a.time<b.time?-1:a.time>b.time?1:0);
  const groups=model.requestEventGroups(m,'g:0');
  assert.equal(groups[0].time,0n);assert.equal(groups[0].events.length,2);
  assert.deepEqual(groups.find(g=>g.time===130n).events.map(e=>e.kind),['received','gw_rx_hold']);
  assert.deepEqual(model.requestEventGroups(m,'copy-a').flatMap(g=>g.events).map(e=>e.node),['gw.b']);
  assert.equal(groups.flatMap(g=>g.events).some(e=>e.kind==='gw_received'),false);
  const gw=model.parseResults(rxHoldingFixture());
  const parent=model.requestEventGroups(gw,'source:0');
  assert.deepEqual(parent.map(g=>g.time),[10n,20n,30n,100n,110n,130n,500n]);
  assert.deepEqual(parent.find(g=>g.time===130n).events.map(e=>e.kind),['observed','received','gw_rx_hold']);
  assert.deepEqual(model.requestEventGroups(gw,'gw:source:0/b').map(g=>g.time),[150n,160n,250n,260n,300n,310n]);
  assert.deepEqual(model.requestEventGroups(gw,'gw:source:0/c').map(g=>g.time),[150n,160n,500n,510n,550n,560n]);
});

test('actual schema2 results replay multiple buses, forwarding boundaries and origin path delay', () => {
  const root=path.resolve(__dirname,'..');
  const bin=process.env.DIR_SIMULATOR_BIN || path.join(root,'target/debug/dir-simulator');
  const temp=fs.mkdtempSync(path.join(os.tmpdir(),'dir-gateway-viewer-'));
  try {
    const results={};
    for (const name of ['delay','independent-only','forward-boundary','hop','queue','capacity-zero','no-route','multicast','multicast-drop','disjoint-cycle','rx-filter','eof-boundary','independent']) {
      const output=path.join(temp,name);
      execFileSync(bin,['run','--config',path.join(root,`docs/verification/fixtures/gw/${name}.ini`),'--output',output]);
      results[name]=JSON.parse(fs.readFileSync(path.join(output,'results.json')));
      assert.doesNotThrow(()=>model.parseResults(results[name]));
    }
    const m=model.parseResults(results.delay);
    assert.deepEqual(m.buses,['Main.busA','Main.busB']);
    assert.deepEqual(m.gateways,[{id:'Main.gw',ports:['Main.gw.a','Main.gw.b']}]);
    assert.deepEqual([...m.gatewayPorts].sort(),['Main.gw.a','Main.gw.b']);
    const configured=structuredClone(results.delay);
    const gatewayConfig=configured.metadata.config.find(entry=>entry.key==='@profile:can.cc.multibus.v1:Main.gw');
    assert(gatewayConfig);
    const normalized=JSON.parse(gatewayConfig.value);
    normalized.ports=['Idle.Controller','Main.gw.a'];
    gatewayConfig.value=JSON.stringify(normalized);
    const configuredModel=model.parseResults(configured);
    assert.deepEqual(configuredModel.gateways,[{id:'Main.gw',ports:['Idle.Controller','Main.gw.a']}]);
    assert.deepEqual([...configuredModel.gatewayPorts].sort(),['Idle.Controller','Main.gw.a']);
    assert(configuredModel.nodes.includes('Idle.Controller'),'configured idle ports remain visible');
    assert.equal(configuredModel.gatewayPorts.has('Main.gw.b'),false,'a Gateway-looking name does not imply membership');
    assert.deepEqual(model.networkAt(configuredModel,0n).gateways,configuredModel.gateways);
    assert.equal(model.forwardStateAt(m.forwards.find(f=>f.egress==='Main.gw.b'),120000000n),'processing');
    assert.equal(model.forwardStateAt(m.forwards.find(f=>f.egress==='Main.gw.b'),126000000n),'submitted');
    const origin=model.originsAt(m,412000000n).find(o=>o.id==='source:0');
    assert.equal(origin.copies,1);assert.equal(origin.nativeStatus,'success');assert.equal(origin.success,1);
    assert.deepEqual(origin.deliveries.map(d=>[d.receiver,d.pathDelay]),[['Main.sink',412000000n]]);
    const simultaneous=model.stateAt(model.parseResults(results['independent-only']),1000000n).buses;
    assert.equal(simultaneous.filter(b=>b.state==='transmitting').length,2);
    const boundary=model.parseResults(results['forward-boundary']);
    assert.equal(model.forwardStateAt(boundary.forwards[0],boundary.end),'processing');
    assert.equal(model.originsAt(boundary,boundary.end)[0].copies,0);
    const boundaryReplay=model.stepTransfers(boundary,boundary.end-1n,boundary.end).filter(item=>item.medium==='gateway');
    assert(boundaryReplay.some(item=>item.kind==='tx'));
    assert(!boundaryReplay.some(item=>item.kind==='rx'),'unfinished Gateway processing cannot invent egress admission');
    const hop=model.parseResults(results.hop);
    assert.equal(hop.forwards.filter(f=>model.forwardStateAt(f,hop.end)==='dropped').length,1);
    const queue=model.parseResults(results.queue);
    const delayed=queue.requests.find(r=>r.txEnqueued !== null && r.ready !== null && r.txEnqueued > r.ready);
    assert(delayed,'actual finite TX queue records delayed admission');
    const transfer=queue.forwards.find(f=>f.child===delayed.id);
    const rx=queue.rxBuffers.find(b=>b.parent===delayed.parent&&b.ingress===transfer.ingress);
    assert.equal(rx.released,delayed.txEnqueued);
    for(const at of [delayed.ready,delayed.txEnqueued-1n]) {
      assert.equal(model.requestStateAt(delayed,at),'waiting_tx');
      assert.equal(model.rxBufferStateAt(rx,at),'holding');
      const state=model.stateAt(queue,at);
      assert(state.nodes.find(n=>n.id===rx.ingress).rxBuffer.held.some(b=>b.parent===delayed.parent));
      assert(!state.nodes.find(n=>n.id===delayed.source).transmitting.includes(delayed.id));
    }
    assert.equal(model.requestStateAt(delayed,delayed.txEnqueued),'pending');
    assert.equal(model.rxBufferStateAt(rx,delayed.txEnqueued),'released');
    const heldState=model.networkAt(queue,delayed.txEnqueued-1n);
    model.networkAt(queue,delayed.txEnqueued);
    assert.deepEqual(model.networkAt(queue,delayed.txEnqueued-1n),heldState);
    const invalid=structuredClone(results.delay);
    invalid.simulation.model_records.find(r=>r.schema_name==='gw.forward' && r.data.egress).data.child_request_id='missing';
    assert.throws(()=>model.parseResults(invalid));
    const unknown=structuredClone(results.delay);unknown.simulation.model_records[0].data.unknown=1;
    assert.throws(()=>model.parseResults(unknown));
    const receiverOrigin=structuredClone(results.delay);
    receiverOrigin.simulation.model_records.find(r=>r.schema_name==='can.receiver').origin_request_id='other:0';
    assert.throws(()=>model.parseResults(receiverOrigin));
    for (const schema of ['can.request','can.receiver','gw.forward']) for (const time of [null,'0']) {
      const timestamp=structuredClone(results.delay);
      timestamp.simulation.model_records.find(r=>r.schema_name===schema).time_ps=time;
      assert.throws(()=>model.parseResults(timestamp),`${schema}: invalid envelope timestamp ${time}`);
    }
  } finally {fs.rmSync(temp,{recursive:true,force:true});}
});
test('actual fanout steps order incoming CAN, internal transfer, outgoing CAN and both receivers',()=>{
  const root=path.resolve(__dirname,'..');
  const bin=process.env.DIR_SIMULATOR_BIN || path.join(root,'target/debug/dir-simulator');
  const temp=fs.mkdtempSync(path.join(os.tmpdir(),'dir-fanout-replay-'));
  try {
    execFileSync(bin,['run','--config',path.join(root,'examples/gateway/fanout.ini'),'--output',path.join(temp,'results')]);
    const raw=JSON.parse(fs.readFileSync(path.join(temp,'results/results.json'))),m=model.parseResults(raw);
    const paired=model.stepTransfers(m,244000000n,248000000n);
    assert.equal(paired.filter(item=>item.medium==='gateway').length,4,'both fanout routes supply distinct TX/RX pairs');
    assert.equal(new Set(paired.filter(item=>item.medium==='gateway').map(item=>item.forwardId)).size,2);
    for(const item of paired) assert.equal(item.stage,item.medium==='gateway'?(item.kind==='tx'?0:1):(item.kind==='tx'?2:3));
    const chain=model.stepTransfers(m,0n,248000000n);
    for(const item of chain) {
      const stage=item.medium==='gateway'?(item.kind==='tx'?2:3):item.requestId==='source:0'?(item.kind==='tx'?0:1):(item.kind==='tx'?4:5);
      assert.equal(item.stage,stage,`${item.requestId} ${item.medium||'can'} ${item.kind} follows its recorded parent`);
      assert.equal(item.untilStage,stage===5?Infinity:stage+1);
    }
    assert.equal(new Set(chain.filter(item=>item.medium==='gateway').map(item=>JSON.stringify([item.gateway,item.routeId,item.source,item.receiver]))).size,2,'identical route IDs never merge distinct egress branches');
    const identities=items=>items.map(item=>[item.medium||'can',item.kind,item.forwardId||item.requestId,item.receiver||null,item.stage]).sort((a,b)=>JSON.stringify(a).localeCompare(JSON.stringify(b)));
    raw.simulation.model_records.reverse();
    assert.deepEqual(identities(model.stepTransfers(model.parseResults(raw),0n,248000000n)),identities(chain),'envelope ordering cannot change causal replay stages');
  }finally{fs.rmSync(temp,{recursive:true,force:true});}
});
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
  const beforeEof=model.networkAt(m,99n,options);
  assert.deepEqual(beforeEof.rx.map(packet=>[packet.requestId,packet.receiver,packet.phase]),[
    ['g:0','Main.b','frame'],['g:0','Main.c','frame']
  ]);
  assert(beforeEof.rx.every(packet=>packet.start===30n&&packet.end===100n));
  assert.equal(model.stateAt(m,99n).counts.received,0);
  assert.equal(model.stateAt(m,99n).counts.rx_pending,0);
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
  assert.deepEqual(model.networkAt(m,99n,options),beforeEof,'rewind restores the pre-EOF frame packets');
  assert.equal(model.networkAt(m,99n,options).trails.length,0);
});
test('event steps include zero-delay TX/RX, preserve physical direction on rewind, and exclude older frames', () => {
  const raw=fixture();
  raw.simulation.receivers.forEach(r=>Object.assign(r,{status:'received',observed_ps:'100',received_ps:'100'}));
  const m=model.parseResults(raw);
  const identities=items=>items.map(r=>[r.kind,r.source,r.receiver||null,r.requestId]);
  const expected=[['tx','Main.a',null,'g:0'],['rx','Main.a','Main.b','g:0'],['rx','Main.a','Main.c','g:0']];
  assert.deepEqual(identities(model.stepTransfers(m,30n,100n)),expected);
  assert.deepEqual(identities(model.stepTransfers(m,100n,30n)),expected);
  assert.deepEqual(identities(model.stepTransfers(m,20n,30n)),expected,'SOF starts the communication even before EOF');
  assert.deepEqual(model.stepTransfers(m,100n,110n),[],'closed communication is not repeated by the next step');
  assert.deepEqual(model.stepTransfers(m,110n,130n),[]);
  assert.deepEqual(model.stepTransfers(m,30n,30n),[]);
  const delayed=model.parseResults(fixture());
  assert.deepEqual(identities(model.stepTransfers(delayed,100n,120n)),expected.slice(1),'observation wait belongs to RX');
  assert.deepEqual(model.stepTransfers(delayed,125n,130n),[],'internal RX processing does not traverse a CAN line');
  const partial=fixture();
  partial.simulation.end_ps='80';partial.simulation.partial=true;
  partial.simulation.requests[0].eof_ps=null;partial.simulation.requests[0].status='in_flight';
  partial.simulation.requests[0].model_fields.release_ps=null;partial.simulation.receivers=[];
  const cutoff=model.parseResults(partial);
  assert.deepEqual(identities(model.stepTransfers(cutoff,30n,80n)),expected.slice(0,1),'planned EOF creates no invented reception');
});

test('zero-delay broadcast completes simultaneously with deterministic visual trails', () => {
  const raw=fixture();
  raw.simulation.receivers.forEach(r=>Object.assign(r,{status:'received',observed_ps:'100',received_ps:'100'}));
  const m=model.parseResults(raw),options={trailWindowPs:20n};
  const before=model.networkAt(m,99n,options),at=model.networkAt(m,100n,options);
  assert.equal(before.tx.length,1);assert.equal(before.trails.length,0);
  assert.deepEqual(before.rx.map(packet=>[packet.receiver,packet.phase]),[['Main.b','frame'],['Main.c','frame']]);
  assert.equal(model.stateAt(m,99n).counts.received,0);
  assert.equal(model.stateAt(m,99n).counts.rx_pending,0);
  assert.equal(at.tx.length,0);assert.equal(at.rx.length,0);
  assert.deepEqual(at.trails.filter(r=>r.kind==='received').map(r=>[r.receiver,r.time,r.opacity]),[['Main.b',100n,1],['Main.c',100n,1]]);
  assert(at.trails.every(r=>r.receiver!==r.source));
  assert.equal(model.stateAt(m,100n).counts.received,2);
  assert(model.networkAt(m,110n,options).trails.every(r=>r.opacity===.5));
  assert.equal(model.networkAt(m,120n,options).trails.length,0);
  assert.equal(model.networkAt(m,100n,{trailWindowPs:0n}).trails.length,0);
});
test('network EOF cutoff does not invent completion or receive records', () => {
  const raw=fixture(),s=raw.simulation,r=s.requests[0];
  s.end_ps='100';s.termination='time_limit';s.receivers=[];
  r.status='in_flight';r.eof_ps=null;r.model_fields.release_ps=null;
  const m=model.parseResults(raw),n=model.networkAt(m,100n);
  assert.equal(m.receivers.length,0);
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

test('declared idle wiring and directed Gateway routes are visible before any event',()=>{
  const m=model.parseResults(rxHoldingFixture()),network=model.networkAt(m,0n);
  assert.equal(network.connections.length,5);
  assert.deepEqual(network.connections.find(c=>c.node==='Idle.Controller'),{node:'Idle.Controller',bus:'Main.busC',inferred:false});
  assert.deepEqual(network.routes.map(r=>[r.ingress,r.egress]),[['Main.gw.a','Main.gw.b'],['Main.gw.a','Main.gw.c']]);
  assert.equal(network.nodes.find(n=>n.id==='Main.source').rxBuffer,undefined,'native CAN Controller has no Gateway RX model');
  assert.equal(network.nodes.find(n=>n.id==='Main.gw.a').rxBuffer.occupancy,0);
  assert.equal(network.tx.length,0);
});
test('fanout holds one RX parent through sibling admission, wait is excluded from TX, and rewind is exact',()=>{
  const m=model.parseResults(rxHoldingFixture());
  const snapshots=new Map();
  for(const at of [129n,130n,159n,160n,240n,250n,499n,500n])snapshots.set(at,model.networkAt(m,at));
  const at160=snapshots.get(160n),ingress=at160.nodes.find(n=>n.id==='Main.gw.a');
  assert.equal(ingress.rxBuffer.occupancy,1);
  assert.deepEqual(ingress.rxBuffer.held.map(b=>b.parent),['source:0']);
  assert.equal(model.stateAt(m,160n).counts.waiting_tx,2);
  assert(at160.nodes.filter(n=>['Main.gw.b','Main.gw.c'].includes(n.id)).every(n=>n.queue===0 && n.waitingTx.length===1));
  assert.deepEqual(at160.transfers.map(t=>t.phase),['waiting_tx','waiting_tx']);
  const at250=snapshots.get(250n);
  assert.equal(at250.nodes.find(n=>n.id==='Main.gw.b').queue,1);
  assert.equal(at250.nodes.find(n=>n.id==='Main.gw.c').queue,0);
  assert.equal(at250.nodes.find(n=>n.id==='Main.gw.a').rxBuffer.occupancy,1);
  assert.deepEqual(at250.transfers.map(t=>t.phase),['pending','waiting_tx']);
  assert.equal(snapshots.get(499n).nodes.find(n=>n.id==='Main.gw.a').rxBuffer.occupancy,1);
  assert.equal(snapshots.get(500n).nodes.find(n=>n.id==='Main.gw.a').rxBuffer.occupancy,0);
  assert.equal(model.rxBufferStateAt(m.rxBuffers[1],239n),'not_created');
  assert.equal(model.rxBufferStateAt(m.rxBuffers[1],240n),'dropped');
  assert.equal(snapshots.get(240n).nodes.find(n=>n.id==='Main.gw.a').rxBuffer.dropped,1);
  for(const [at,snapshot] of [...snapshots].reverse())assert.deepEqual(model.networkAt(m,at),snapshot);
  assert(m.eventTimes.includes(250n) && m.eventTimes.includes(500n));
});
test('Gateway replay keeps blocked fanout siblings at ingress and receives only an admitted branch',()=>{
  const m=model.parseResults(rxHoldingFixture());
  const incoming=model.stepTransfers(m,129n,130n);
  const incomingRx=incoming.find(item=>item.medium!=='gateway'&&item.kind==='rx'&&item.requestId==='source:0');
  assert.equal(incomingRx.stage,0);
  const initial=incoming.filter(item=>item.medium==='gateway');
  assert.equal(initial.length,2);assert(initial.every(item=>item.kind==='tx'&&item.stage===1));
  assert.equal(model.stepTransfers(m,160n,240n).filter(item=>item.medium==='gateway').length,0,'capacity wait does not traverse a route');
  const first=model.stepTransfers(m,249n,250n).filter(item=>item.medium==='gateway');
  assert.deepEqual(first.map(item=>[item.kind,item.receiver,item.stage]),[['tx','Main.gw.b',0],['rx','Main.gw.b',1]]);
  const outgoing=model.stepTransfers(m,249n,260n);
  assert.equal(outgoing.find(item=>item.medium!=='gateway'&&item.kind==='tx'&&item.requestId==='gw:source:0/b').stage,2);
  assert(!outgoing.some(item=>item.medium==='gateway'&&item.receiver==='Main.gw.c'),'blocked sibling receives no copied traffic');
  const second=model.stepTransfers(m,499n,500n).filter(item=>item.medium==='gateway');
  assert.deepEqual(second.map(item=>[item.kind,item.receiver,item.stage]),[['tx','Main.gw.c',0],['rx','Main.gw.c',1]]);
  const before=model.gatewayTransfersAt(m,149n).find(item=>item.receiver==='Main.gw.b');
  const after=model.gatewayTransfersAt(m,150n).find(item=>item.receiver==='Main.gw.b');
  assert.equal(before.phase,'processing');assert.equal(after.phase,'processing');
  assert(after.progress>before.progress,'Gateway processing and egress TX processing share one continuous logical path');
  assert.equal(after.admitted,false);assert.equal(model.gatewayTransfersAt(m,250n).find(item=>item.receiver==='Main.gw.b').admitted,true);
});
test('schema2 holding cutoff, no-route immediate release, legacy fields and strict RX validation',()=>{
  const raw=rxHoldingFixture();
  raw.simulation.model_records=raw.simulation.model_records.filter(row=>row.origin_request_id==='source:0');
  for(const row of raw.simulation.model_records) {
    if(row.schema_name==='can.request' && row.data.model_fields.parent_request_id) {
      Object.assign(row.data,{status:'waiting_tx',sof_ps:null,eof_ps:null});
      Object.assign(row.data.model_fields,{tx_enqueued_ps:null,planned_eof_ps:null,planned_release_ps:null,release_ps:null});row.time_ps='160';
    }
    if(row.schema_name==='gw.rx_buffer'){Object.assign(row.data,{status:'holding',released_ps:null});row.time_ps='130';}
  }
  raw.simulation.end_ps='200';raw.simulation.termination='time_limit';
  const m=model.parseResults(raw);
  assert.equal(model.stateAt(m,200n).counts.waiting_tx,2);
  assert.equal(model.stateAt(m,200n).nodes.find(n=>n.id==='Main.gw.a').rxBuffer.occupancy,1);
  const noRoute=structuredClone(raw);
  noRoute.simulation.model_records=noRoute.simulation.model_records.filter(r=>r.schema_name==='gw.rx_buffer'||r.schema_name==='can.receiver'||(r.schema_name==='can.request'&&r.data.request_id==='source:0'));
  const b=noRoute.simulation.model_records.find(r=>r.schema_name==='gw.rx_buffer');
  Object.assign(b.data,{status:'released',released_ps:'130',egress:[]});
  assert.equal(model.stateAt(model.parseResults(noRoute),130n).nodes.find(n=>n.id==='Main.gw.a').rxBuffer.occupancy,0);
  const legacy=rxHoldingFixture();delete legacy.metadata.topology;
  legacy.simulation.model_records=legacy.simulation.model_records.filter(row=>row.schema_name!=='gw.rx_buffer');
  for(const row of legacy.simulation.model_records)if(row.schema_name==='can.request')delete row.data.model_fields.tx_enqueued_ps;
  assert.doesNotThrow(()=>model.parseResults(legacy));
  for(const change of [r=>r.subject='wrong',r=>r.data.capacity='4294967296',r=>r.data.released_ps='129',r=>{r.data.released_ps='250';r.time_ps='250';},r=>r.data.egress=['Main.gw.b'],r=>r.data.reason='rx_queue_full',r=>r.time_ps='0',r=>r.data.unknown=true]) {
    const invalid=rxHoldingFixture();change(invalid.simulation.model_records.find(r=>r.schema_name==='gw.rx_buffer'));assert.throws(()=>model.parseResults(invalid));
  }
});
test('schema1 declared wiring retains an idle Controller without guessing Gateway membership',()=>{
  const raw=fixture();raw.metadata.topology={controllers:[{id:'Idle.Controller',bus:'Main.bus',tx_channel_delay_ps:'7',rx_channel_delay_ps:'9'}]};
  const m=model.parseResults(raw),network=model.networkAt(m,0n);
  assert.deepEqual(network.connections.find(c=>c.node==='Idle.Controller'),{node:'Idle.Controller',bus:'Main.bus',inferred:false});
  assert.equal(network.nodes.find(n=>n.id==='Idle.Controller').rxBuffer,undefined);
});

test('legacy Gateway membership never guesses idle CAN wiring from a single observed bus',()=>{
  const raw=rxHoldingFixture();delete raw.metadata.topology;
  raw.simulation.model_records=[];raw.simulation.records=[];raw.simulation.end_ps='0';raw.simulation.termination='time_limit';
  raw.metadata.config.push({key:'Main.busA.bitrate',value:'500000'});
  const network=model.networkAt(model.parseResults(raw),0n);
  assert.equal(network.nodes.length,3);
  assert.deepEqual(network.connections,[]);
});
