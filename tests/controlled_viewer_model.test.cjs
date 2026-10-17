const test=require('node:test'),assert=require('node:assert/strict');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/controlled-model.js');
function fixture(){return {schema_version:3,metadata:{model_profile:'custom.v1',model_schemas:[{schema_name:'state',schema_version:1}],metrics_v2:[{name:'gauge',unit:'count',version:'1',value_kind:'integer',sampling:'gauge',aggregation:'time_mean',targets:['n'],initial_gauges:{n:'2'}}]},control:{version:'1',session_id:'s',export_ordinal:'0',state:'Paused',commit_ordinal:'2',horizon_ps:'7',pending_events:'1',resume_supported:true,replay_sha256:'0'.repeat(64)},simulation:{end_ps:'7',committed_events:'2',termination:'paused',model_records:[{schema_name:'state',schema_version:1,record_id:'r',subject:'n',time_ps:'7',data:{phase:'committed'}}],records:[{seq:'0',descriptor_version:'1',event_seq:'0',effect_seq:'0',metric:'gauge',target:'n',unit:'count',value:'4',time_ps:'3'}],summary:[{metric:'gauge',target:'n',aggregation:'time_mean',unit:'count',value_kind:'number',value:22/7,sample_count:'1',start_ps:null,end_ps:null}]}};}
test('controlled prefix includes H records without inventing earlier actual state',()=>{const m=M.parseResults(fixture());assert.equal(M.at(m,6n).events.length,0);assert.equal(M.at(m,7n).events.length,1);assert.equal(M.at(m,2n).metrics[0].value,'2');assert.equal(M.at(m,2n).metrics[0].initial,true);assert.equal(M.at(m,3n).metrics[0].value,'4');assert.equal(M.at(m,6n).events.length,0);});
test('control boundary mismatch and unsupported revision fail',()=>{for(const [field,value] of [['horizon_ps','8'],['commit_ordinal','3'],['version','2'],['resume_supported',false],['replay_sha256','x']]){const f=fixture();f.control[field]=value;assert.throws(()=>M.parseResults(f));}});
test('unknown schemas, duplicate record IDs and future records fail',()=>{for(const mutation of [f=>f.simulation.model_records[0].schema_name='unknown',f=>f.simulation.model_records.push({...f.simulation.model_records[0]}),f=>f.simulation.model_records[0].time_ps='8']){const f=fixture();mutation(f);assert.throws(()=>M.parseResults(f));}});
test('metric type, target, version and overflow fail',()=>{for(const [field,value] of [['target','other'],['descriptor_version','2'],['value','18446744073709551616'],['time_ps','07']]){const f=fixture();f.simulation.records[0][field]=value;assert.throws(()=>M.parseResults(f));}});
test('summary finite numbers and half-open window range are validated',()=>{let f=fixture();f.simulation.summary[0].value=Infinity;assert.throws(()=>M.parseResults(f));f=fixture();f.simulation.summary[0].start_ps='6';f.simulation.summary[0].end_ps='8';assert.throws(()=>M.parseResults(f));});
test('fragment replay excludes suspension and planned ends, and is reversible',()=>{
 const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.preemption.abstract.v1';f.metadata.metrics_v2=[];f.simulation.end_ps='1000';f.simulation.records=[];f.simulation.summary=[];
 f.metadata.model_schemas=['dir.preemption.transfer','dir.preemption.fragment'].map(schema_name=>({schema_name,schema_version:1}));
 const row=(kind,id,time,data)=>({schema_name:kind,schema_version:1,record_id:id,subject:'port',time_ps:time,data});
 f.simulation.model_records=[row('dir.preemption.transfer','t','900',{transfer_id:'t',queued_ps:'0',sof_ps:'0',eof_ps:'900',release_ps:'900',kind:'preemptable',mac_bytes:'80',completed_mac_bytes:'80',occupied_ps:'800',status:'completed'}),row('dir.preemption.fragment','f1','400',{transfer_id:'t',start_ps:'0',end_ps:'400',payload_bytes:'40',status:'completed'}),row('dir.preemption.fragment','f2','900',{transfer_id:'t',start_ps:'500',end_ps:'900',planned_end_ps:'900',payload_bytes:'40',status:'completed'})];
 const m=M.parseResults(f);let state=M.at(m,600n),t=state.events.find(e=>e.id==='t');assert.equal(t.data.occupied_ps,'500');assert.equal(t.data.completed_mac_bytes,'40');assert.equal(t.data.eof_ps,null);assert.equal(state.events.find(e=>e.id==='f2').data.end_ps,null);
 assert.equal(M.at(m,300n).events.find(e=>e.id==='t').data.occupied_ps,'300');assert.deepEqual(M.at(m,600n),state);
 const partial=structuredClone(f);partial.simulation.end_ps='900';const transfer=partial.simulation.model_records[0];transfer.time_ps='500';transfer.data.eof_ps=null;transfer.data.release_ps=null;transfer.data.status='in_flight';const fragment=partial.simulation.model_records[2];fragment.time_ps='500';fragment.data.end_ps=null;fragment.data.status='in_flight';
 const prefix=M.at(M.parseResults(partial),900n);assert.equal(prefix.events.find(e=>e.id==='f2').data.end_ps,null);assert.equal(prefix.events.find(e=>e.id==='t').data.eof_ps,null);
});

test('builtin profile cannot smuggle an unsupported schema through its metadata catalog',()=>{const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.preemption.abstract.v1';assert.throws(()=>M.parseResults(f),/schema/);});

test('redundancy rewind follows actual parent transfers instead of the final hop cursor',()=>{
 const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.redundancy.abstract.v1';f.metadata.metrics_v2=[];f.simulation.end_ps='1000';f.simulation.records=[];f.simulation.summary=[];
 const row=(kind,id,time,data)=>({schema_name:kind,schema_version:1,record_id:id,subject:'port',time_ps:time,data});
 f.metadata.model_schemas=['dir.redundancy.branch','dir.redundancy.origin','ethernet.dynamic.transfer'].map(schema_name=>({schema_name,schema_version:1}));
 f.simulation.model_records=[row('dir.redundancy.origin','o','900',{source_frame_id:'frame',created_ps:'0',first_sof_ps:'0',completed_ps:'900',completed_count:'1',target_count:'1'}),row('dir.redundancy.branch','b','900',{origin_id:'o',stream_id:'stream',path_id:'path',path_ports:['a.tx','s.tx'],created_ps:'0',first_sof_ps:'0',completed_ps:'900',status:'delivered',hop_index:'1',current_transfer_id:null,recovery_id:'r',reason:null}),row('ethernet.dynamic.transfer','t0','300',{frame_id:'frame',from_port:'a.tx',parent_transfer_id:null,queued_ps:'0',sof_ps:'0',eof_ps:'100',arrival_ps:'150',release_ps:'200',status:'completed'}),row('ethernet.dynamic.transfer','t1','900',{frame_id:'frame',from_port:'s.tx',parent_transfer_id:'t0',queued_ps:'300',sof_ps:'400',eof_ps:'500',arrival_ps:'700',release_ps:'600',status:'completed'})];
 const m=M.parseResults(f),at=t=>M.at(m,BigInt(t)).events.find(e=>e.id==='b').data;
 assert.equal(at(100).hop_index,'0');assert.equal(at(100).current_transfer_id,'t0');assert.equal(at(350).hop_index,'1');assert.equal(at(350).status,'queued');assert.equal(at(450).status,'in_flight');assert.equal(at(950).current_transfer_id,null);assert.deepEqual(at(100),at(100));
});

test('explicit circuit provenance retains a queued branch before its later pre-SOF drop',()=>{
 const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.redundancy.abstract.v1';f.metadata.metrics_v2=[];f.simulation.end_ps='1000';f.simulation.records=[];f.simulation.summary=[];
 const row=(kind,id,time,data)=>({schema_name:kind,schema_version:1,record_id:id,subject:'port',time_ps:time,data});
 f.metadata.model_schemas=['dir.redundancy.branch','dir.redundancy.origin','ethernet.dynamic.transfer'].map(schema_name=>({schema_name,schema_version:1}));
 f.metadata.network_runtime={redundancy:{transfer_circuits:[{branch_id:'b',transfer_id:'t',hop_index:'0'}]}};
 f.simulation.model_records=[row('dir.redundancy.origin','o','1000',{source_frame_id:'frame',created_ps:'0',first_sof_ps:null,completed_ps:null,completed_count:'0',target_count:'0'}),row('dir.redundancy.branch','b','1000',{origin_id:'o',path_ports:['a.tx'],created_ps:'0',first_sof_ps:null,completed_ps:'1000',status:'dropped',hop_index:'0',current_transfer_id:null,recovery_id:null,reason:'link_down'}),row('ethernet.dynamic.transfer','t','1000',{frame_id:'frame',from_port:'a.tx',parent_transfer_id:null,queued_ps:'0',sof_ps:null,status:'dropped',drop_reason:'link_down'})];
 const m=M.parseResults(f),before=M.at(m,500n).events.find(e=>e.id==='b').data;assert.equal(before.status,'queued');assert.equal(before.current_transfer_id,'t');assert.equal(before.reason,null);assert.equal(M.at(m,1000n).events.find(e=>e.id==='b').data.status,'dropped');
 for(const mutate of [x=>x.metadata.network_runtime.redundancy.transfer_circuits[0].hop_index='1',x=>x.metadata.network_runtime.redundancy.transfer_circuits.push({...x.metadata.network_runtime.redundancy.transfer_circuits[0]}),x=>x.simulation.model_records[2].data.frame_id='other',x=>x.simulation.model_records[2].data.parent_transfer_id='t']){const bad=structuredClone(f);mutate(bad);assert.throws(()=>M.parseResults(bad));}
});

// Selected unmodified records from independent real full/prefix CLI executions.
// Source result digests and the selection rule are retained in the fixture.
const rewindFixture=require('./fixtures/controlled-rewind.json');
const actualReplay=(name,time)=>M.at(M.parseResults(rewindFixture[name]),BigInt(time));
test('actual preemption rewind hides future fragment plans and references until the last fragment starts',()=>{
 const full=actualReplay('full-run',600000),prefix=actualReplay('prefix-run',600000);
 const transfer=s=>s.events.find(e=>e.kind==='dir.preemption.transfer'&&e.id==='slow:0/v0/c0').data;
 const fields=['planned_eof_ps','planned_release_ps','planned_arrival_ps','fragment_ids','gap_ids','occupied_ps','completed_mac_bytes','status'];
 for(const field of fields)assert.deepEqual(transfer(full)[field],transfer(prefix)[field],field);
 assert.equal(transfer(full).planned_eof_ps,null);assert.equal(transfer(full).fragment_ids.length,1);assert.equal(transfer(full).gap_ids.length,0);
 assert.equal(transfer(actualReplay('full-run',1383999)).planned_eof_ps,null);
 assert.equal(transfer(actualReplay('full-run',1384000)).planned_eof_ps,'1576000');
 assert.deepEqual(actualReplay('full-run',600000),full);
});
test('actual reception rewind retains the observed processing reception without later relay effects',()=>{
 const full=actualReplay('redundancy-full-run',610000),prefix=actualReplay('redundancy-prefix-run',610000);
 const reception=s=>s.events.find(e=>e.kind==='ethernet.dynamic.reception'&&e.id==='unicast:0/v0/c0@rx').data;
 for(const field of ['observed_ps','ready_ps','planned_ready_ps','status','reason','egress_transfer_ids'])assert.deepEqual(reception(full)[field],reception(prefix)[field],field);
 assert.equal(reception(full).status,'processing');assert.equal(reception(full).ready_ps,null);assert.deepEqual(reception(full).egress_transfer_ids,[]);
 assert.equal(actualReplay('redundancy-full-run',608999).events.some(e=>e.id==='unicast:0/v0/c0@rx'),false);
});
test('actual delivery rewind retains frame-born frozen opportunities and hides future completions',()=>{
 const deliveries=s=>s.events.filter(e=>e.kind==='dir.preemption.delivery').map(e=>({id:e.id,data:Object.fromEntries(['frame_id','source_transfer_id','first_sof_ps','target_count','completed_count','targets'].map(k=>[k,e.data[k]]))}));
 const full=deliveries(actualReplay('full-run',600000)),prefix=deliveries(actualReplay('prefix-run',600000));
 assert.deepEqual(full,prefix);assert.equal(full.length,2);assert.ok(full.every(e=>e.data.completed_count==='0'));
 assert.equal(deliveries(actualReplay('full-run',99999)).length,1);
});
