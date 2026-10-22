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
test('PAUSE replay orders same-cut replacement after old expiry and never borrows a future timer',()=>{
 const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.network.composite.v1';f.metadata.metrics_v2=[];f.simulation.end_ps='40';f.simulation.records=[];f.simulation.summary=[];
 f.metadata.model_schemas=['dir.network.mac_control','dir.network.configured_media'].map(schema_name=>({schema_name,schema_version:1}));
 const state=timer=>({link_up:true,transmit_enabled:true,rx_enabled:true,tx_enabled:true,pending:null,timer});
 const timer=(loaded,expires)=>({loaded_ps:loaded,expires_ps:expires,request:{received_ps:loaded}});
 const row=(id,time,seq,value)=>({schema_name:'dir.network.mac_control',schema_version:1,record_id:id,subject:'a.tx',time_ps:time,data:{port:'a.tx',kind:'timer_load',effect_seq:seq,state:value,evidence:{}}});
 f.simulation.model_records=[row('a-new','20','3',state(timer('20','30'))),row('z-old','10','1',state(timer('10','20'))),row('b-expiry','20','2',state(null))];
 const m=M.parseResults(f),at=t=>M.pausePorts(M.at(m,BigInt(t)).events,BigInt(t));
 assert.deepEqual(at(9),[]);assert.equal(at(19)[0].remaining_ps,'1');assert.equal(at(20)[0].paused,true);assert.equal(at(20)[0].remaining_ps,'10');assert.equal(at(30)[0].paused,false);assert.deepEqual(at(19),at(19));
});
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

test('composite catalog accepts shipped Qcc and active wire schemas while keeping unknown rejection',()=>{
 const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.network.composite.v1';f.metadata.metrics_v2=[];f.simulation.records=[];f.simulation.summary=[];
 const names=['dir.network.qcc','dir.network.segment','dir.network.provider','ethernet.wire.boundary','ethernet.wire.membership','ethernet.wire.membership_timer','ethernet.control.wire.event','ethernet.control.wire.policy','ethernet.control.wire.transition','ethernet.control.wire.tree_policy'];
 f.metadata.model_schemas=names.map(schema_name=>({schema_name,schema_version:1}));
 f.simulation.model_records=[{schema_name:'dir.network.qcc',schema_version:1,record_id:'prepare',subject:'network',time_ps:'3',data:{kind:'prepared',transaction:'install',activate_at_ps:'6',effect_seq:'0'}},{schema_name:'dir.network.qcc',schema_version:1,record_id:'installed',subject:'network',time_ps:'6',data:{kind:'installed',transaction:'install',version:'1',effect_seq:'1'}}];
 const model=M.parseResults(f);assert.deepEqual(M.at(model,5n).events.map(e=>e.data.kind),['prepared']);assert.deepEqual(M.at(model,6n).events.map(e=>e.data.kind),['prepared','installed']);assert.deepEqual(M.at(model,5n).events.map(e=>e.data.kind),['prepared']);
 f.metadata.model_schemas.push({schema_name:'dir.network.unknown',schema_version:1});assert.throws(()=>M.parseResults(f),/schema/);
});

// Independently generated literal zlib CRC fixture also used by the Rust L2 gate.
function stackedWire(){return {tag:null,tags:[{tpid:'34984',tci:{vid:'100',pcp:'5',dei:'1'}},{tpid:'33024',tci:{vid:'10',pcp:'3',dei:'0'}}],mac_bytes:'72',mac_hex:'02000000000202000000000188a8b0648100600a0800000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d0ad92a3a',fcs_hex:'0ad92a3a'};}
test('ordered S/C tag metadata matches literal TPID TCI and actual FCS',()=>{
 assert.deepEqual(M.tagStack(stackedWire()).map(t=>[t.kind,t.vid,t.pcp,t.dei]),[['S','100','5','1'],['C','10','3','0']]);
 assert.deepEqual(M.tagStack({tag:{vid:'10',pcp:'3',dei:'0'}}).map(t=>[t.kind,t.vid]),[['C','10']]);
 for(const mutate of [w=>w.tag={vid:'10',pcp:'3',dei:'0'},w=>w.tags=[],w=>w.tags.reverse(),w=>w.tags[0].tpid='33024',w=>w.tags[0].tpid='034984',w=>w.tags[0].tci.vid='4095',w=>w.tags[1].tci.pcp='8',w=>w.tags[1].tci.dei='2',w=>w.tags[0].extra=true,w=>w.mac_bytes='68',w=>w.mac_hex=w.mac_hex.replace('88a8b064','88a8b065'),w=>w.fcs_hex='00000000',w=>w.mac_hex=w.mac_hex.slice(0,-2)+'00']){
  const wire=stackedWire();mutate(wire);assert.throws(()=>M.tagStack(wire));
 }
 const f=fixture();f.simulation.model_records[0].data.wire=stackedWire();assert.doesNotThrow(()=>M.parseResults(f));f.simulation.model_records[0].data.wire.tags[0].tci.vid='101';assert.throws(()=>M.parseResults(f),/tag/);
});

test('distributed FRER rewind allocates after actual prefix processing and follows physical branches',()=>{
 const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.frer.rtag.v1';f.metadata.metrics_v2=[];f.simulation.end_ps='2700000';f.simulation.records=[];f.simulation.summary=[];
 const row=(kind,id,time,data)=>({schema_name:kind,schema_version:1,record_id:id,subject:'port',time_ps:String(time),data});
 f.metadata.model_schemas=['dir.redundancy.branch','dir.redundancy.origin','ethernet.dynamic.transfer','ethernet.dynamic.reception'].map(schema_name=>({schema_name,schema_version:1}));
 const id=i=>`frame/v0/c${i}`,phase=(current,sof,terminal)=>({current_transfer:current,first_sof_ps:sof,terminal_ps:terminal,failure_reason:null});
 const origin={origin_id:'flow',placement:'bridge_pair_v1',source_frame_id:'frame',sequence:'0',created_ps:'0',first_sof_ps:'0',completed_ps:'2541000',completed_count:'1',target_count:'1',target:'Net.d',prefix_reception_id:`${id(0)}@rx`,winner_reception_id:`${id(3)}@rx`,delivery_reception_id:`${id(5)}@rx`,continuation_created:true,continuation_ps:'1929000',suffix_root:'5',prefix_ports:['Net.s.tx'],branch_ports:[['Net.r.tx_a','Net.a.tx_e'],['Net.r.tx_b','Net.b.tx_e']],suffix_ports:['Net.e.tx_d'],prefix:phase('0','0','611000'),branches:[phase('3','611000','1929000'),phase('4','611000','1929000')],suffix:phase('5','1929000','2541000')};
 const specs=[['prefix',null,0,'Net.s.tx',null,'0'],['branch',0,0,'Net.r.tx_a',0,'611000'],['branch',1,0,'Net.r.tx_b',0,'611000'],['branch',0,1,'Net.a.tx_e',1,'1270000'],['branch',1,1,'Net.b.tx_e',2,'1270000'],['suffix',null,0,'Net.e.tx_d',3,'1929000']];
 f.metadata.network_runtime={redundancy:{transfer_circuits:specs.map(([phase,path,hop],i)=>({branch_id:phase==='branch'?`b${path}`:'flow',transfer_id:id(i),hop_index:String(hop),distributed:{flow:'flow',leg:{phase,...(path===null?{}:{path})}}}))}};
 f.simulation.model_records=[row('dir.redundancy.origin','flow',2541000,origin),...specs.map(([leg,path,hop,port,parent,queued],i)=>row('ethernet.dynamic.transfer',id(i),2600000,{frame_id:'frame',from_port:port,parent_transfer_id:parent===null?null:id(parent),queued_ps:queued,sof_ps:queued,status:'completed'}))];
 for(const [i,ready,egress] of [[0,611000,[1,2]],[1,1270000,[3]],[2,1270000,[4]],[3,1929000,[5]],[4,1929000,[]],[5,2541000,[]]])f.simulation.model_records.push(row('ethernet.dynamic.reception',`${id(i)}@rx`,ready,{observed_ps:String(ready-2000),ready_ps:String(ready),status:egress.length?'forwarded':'received',egress_transfer_ids:egress.map(id)}));
 for(const path of [0,1])f.simulation.model_records.push(row('dir.redundancy.branch',`b${path}`,2541000,{origin_id:'flow',stream_id:'stream',path_id:String(path),path_ports:origin.branch_ports[path],created_ps:'611000',first_sof_ps:'611000',completed_ps:'1929000',status:path?'duplicate':'accept_first',hop_index:'1',current_transfer_id:null,recovery_id:`recovery${path}`,reason:null}));
 const model=M.parseResults(f),state=t=>M.at(model,BigInt(t)),o=t=>state(t).events.find(e=>e.id==='flow').data;
 for(const t of [0,609000]){assert.equal(o(t).sequence,null);assert.equal(o(t).prefix_reception_id,null);assert.deepEqual(o(t).branches,[]);assert.equal(o(t).suffix.current_transfer,null);}
 assert.equal(o(611000).sequence,'0');assert.equal(o(611000).prefix_reception_id,`${id(0)}@rx`);
 const branch=t=>state(t).events.find(e=>e.id==='b0').data;
 assert.equal(branch(611000).current_transfer_id,id(1));assert.equal(branch(1268000).hop_index,'0');assert.equal(branch(1270000).current_transfer_id,id(3));
 assert.equal(o(1929000).continuation_created,true);assert.equal(o(1929000).delivery_reception_id,null);assert.equal(branch(1929000).status,'accept_first');assert.equal(branch(1929000).current_transfer_id,null);
 assert.equal(o(2541000).completed_count,'1');assert.equal(o(2541000).delivery_reception_id,`${id(5)}@rx`);
});

test('normative endpoint delivery indications replay each accepted delivery independently',()=>{
 const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.frer.rtag.v1';f.metadata.metrics_v2=[];f.metadata.model_schemas=[{schema_name:'dir.redundancy.origin',schema_version:1}];f.simulation.end_ps='400';f.simulation.records=[];f.simulation.summary=[];
 f.simulation.model_records=[{schema_name:'dir.redundancy.origin',schema_version:1,record_id:'origin',subject:'port',time_ps:'300',data:{created_ps:'0',source_frame_id:'frame',first_sof_ps:'0',completed_ps:'100',completed_count:'2',target_count:'1',delivered_branch_id:'a',delivery_indications:[{recovery_id:'ra',reception_id:'rxa',branch_id:'a',delivered_ps:'100'},{recovery_id:'rb',reception_id:'rxb',branch_id:'b',delivered_ps:'300'}]}}];
 const m=M.parseResults(f),at=t=>M.at(m,BigInt(t)).events.find(e=>e.id==='origin').data;
 assert.equal(at(99).completed_count,'0');assert.equal(at(100).completed_count,'1');assert.equal(at(299).completed_count,'1');assert.equal(at(300).completed_count,'2');assert.deepEqual(at(150),at(150));
});


test('serializer reset replay retains aborted payload and successful prior RX without future completion',()=>{
 const f=fixture();f.schema_version=2;delete f.control;f.metadata.model_profile='ethernet.network.composite.v1';f.metadata.metrics_v2=[];f.simulation.end_ps='40';f.simulation.records=[];f.simulation.summary=[];
 f.metadata.model_schemas=['dir.preemption.transfer','dir.preemption.fragment'].map(schema_name=>({schema_name,schema_version:1}));
 const row=(kind,id,time,data)=>({schema_name:kind,schema_version:1,record_id:id,subject:'port',time_ps:String(time),data});
 f.simulation.model_records=[
  row('dir.preemption.transfer','partial',30,{transfer_id:'partial',queued_ps:'0',sof_ps:'10',eof_ps:null,release_ps:null,arrival_ps:null,reset_ps:'17',arrival_retired_ps:'30',kind:'preemptable',mac_bytes:'10',completed_mac_bytes:'0',partial_mac_bytes:'3',occupied_ps:'7',status:'dropped',drop_reason:'link_epoch_reset'}),
  row('dir.preemption.fragment','aborted',17,{transfer_id:'partial',start_ps:'10',planned_end_ps:'22',end_ps:'17',offset_bytes:'0',payload_bytes:'10',overhead_bytes:'2',bitrate_bps:'8000000000000',emitted_payload_bytes:'3',status:'aborted'}),
  row('dir.preemption.transfer','received',30,{transfer_id:'received',queued_ps:'0',sof_ps:'0',eof_ps:'20',release_ps:null,arrival_ps:'22',reset_ps:'25',kind:'express',mac_bytes:'10',completed_mac_bytes:'10',partial_mac_bytes:'0',occupied_ps:'25',status:'completed',drop_reason:null}),
 ];
 const m=M.parseResults(f),at=t=>M.at(m,BigInt(t)),get=(t,id)=>at(t).events.find(e=>e.id===id).data;
 assert.equal(get(16,'partial').status,'in_flight');assert.equal(get(16,'partial').reset_ps,null);assert.equal(get(16,'partial').partial_mac_bytes,'0');assert.equal(get(16,'aborted').status,'in_flight');assert.equal(get(16,'aborted').emitted_payload_bytes,undefined);
 assert.equal(get(17,'partial').status,'dropped');assert.equal(get(17,'partial').partial_mac_bytes,'3');assert.equal(get(17,'partial').completed_mac_bytes,'0');assert.equal(get(17,'aborted').status,'aborted');assert.equal(get(17,'aborted').emitted_payload_bytes,'3');
 assert.equal(get(39,'partial').occupied_ps,'7');assert.equal(get(39,'partial').completed_mac_bytes,'0');assert.equal(get(19,'received').completed_mac_bytes,'0');assert.equal(get(25,'received').arrival_ps,'22');assert.equal(get(25,'received').status,'completed');assert.equal(get(39,'received').occupied_ps,'25');assert.equal(get(39,'received').completed_mac_bytes,'10');
 assert.deepEqual(at(16),at(16));assert.deepEqual(at(17),at(17));
});
