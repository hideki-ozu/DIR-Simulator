'use strict';
const test=require('node:test');
const assert=require('node:assert/strict');
const E=require('../crates/dir-simulator/src/tool/viewer/assets/ethernet-model.js');
const {vlanFixture}=require('./ethernet_viewer_model.test.cjs');
const schedule={id:'g0',base_time_ps:'0',cycle_time_ps:'2000000',entries:[{duration_ps:'1000000',open_priorities:[7]},{duration_ps:'1000000',open_priorities:[0]}]};
test('GCL uses half-open boundaries and preserves exact time beyond Number range',()=>{
 assert.deepEqual(E.scheduleStateAt(schedule,999999n),{open:[7],next:1000000n});
 assert.deepEqual(E.scheduleStateAt(schedule,1000000n),{open:[0],next:2000000n});
 assert.deepEqual(E.scheduleStateAt({...schedule,base_time_ps:'9007199254740993'},9007199254740992n),{open:[],next:9007199254740993n});
 assert.deepEqual(E.scheduleStateAt(null,1000000n),{open:[0,1,2,3,4,5,6,7],next:null});
});
test('open duration handles millions of cycles with integer prefix arithmetic',()=>{
 assert.equal(E.openElapsed(schedule,7,672000n,3688000n),1328000n);
 const end=9007199254740993n;const whole=end/2000000n,phase=end%2000000n;
 assert.equal(E.openElapsed(schedule,7,0n,end),whole*1000000n+(phase<1000000n?phase:1000000n));
});
test('CBS continues through IFG, freezes at closed gates and resets empty positive credit',()=>{
 const idle=250000000n,send=idle-1000000000n;
 assert.equal(E.creditNumeratorAt(0n,send,0n,576000n,{sending:true}),-432000000000000n);
 assert.equal(E.creditNumeratorAt(0n,send,0n,672000n,{sending:true}),-504000000000000n);
 assert.equal(E.creditNumeratorAt(-504000000000000n,idle,672000n,1000000n,{schedule,priority:7}),-422000000000000n);
 assert.equal(E.creditNumeratorAt(-422000000000000n,idle,1000000n,2000000n,{schedule,priority:7}),-422000000000000n);
 assert.equal(E.creditNumeratorAt(-504000000000000n,idle,672000n,2688000n,{backlog:false}),0n);
 assert.equal(E.creditNumeratorAt(1n,idle,0n,1n,{schedule,priority:0,backlog:false}),0n);
 assert.equal(E.creditNumeratorAt(100n,idle,0n,1000000n,{hi:200n}),200n);
});
test('meter potential refill saturates u128 without inventing token consumption',()=>{
 const cap=(1n<<128n)-1n;assert.equal(E.potentialTokens(cap-1n,1000000000n,0n,9007199254740993n,cap),cap);
 assert.equal(E.potentialTokens(0n,1000000n,0n,200000000n,640000000000000n),200000000000000n);
 assert.throws(()=>E.potentialTokens(0n,1n,2n,1n,10n),/Ethernet:/);
});
test('malformed schedules fail before replay',()=>{
 for(const mutate of [s=>s.entries[0].open_priorities=[7,7],s=>s.entries[0].open_priorities=[8],s=>s.entries[0].duration_ps='0',s=>s.cycle_time_ps='1',s=>s.extra=true]){
  const s=structuredClone(schedule);mutate(s);assert.throws(()=>E.prepareSchedule(s),/Ethernet:/);
 }
});
test('TAS fit includes IFG and accepts equality at close or a pending update',()=>{
 const s={id:'fit',base_time_ps:'0',cycle_time_ps:'2000000',entries:[{duration_ps:'800000',open_priorities:[7]},{duration_ps:'1200000',open_priorities:[]}]};
 assert.equal(E.nextGateFit(s,7,128000n,672000n),128000n);
 assert.equal(E.nextGateFit(s,7,128001n,672000n),2000000n);
 assert.equal(E.nextGateFit(s,7,0n,672000n,672000n),0n);
 assert.equal(E.nextGateFit(s,7,0n,672000n,671999n),null);
 assert.equal(E.nextGateFit(s,7,0n,800001n),null);
});
test('TAS combines adjacent open entries and the end/start of a cycle',()=>{
 const s={id:'wrap',base_time_ps:'0',cycle_time_ps:'3000000',entries:[{duration_ps:'1000000',open_priorities:[7]},{duration_ps:'1000000',open_priorities:[]},{duration_ps:'1000000',open_priorities:[7]}]};
 assert.equal(E.nextGateFit(s,7,2500000n,1500000n),2500000n);
 assert.equal(E.nextGateFit(s,7,2500001n,1500000n),5000000n);
 assert.equal(E.nextGateFit(s,7,0n,1500000n),2000000n);
});
test('IP membership identity uses address bytes and multicast MAC mapping',()=>{
 const ipv4={family:'ipv4',source:'192.0.2.1',group:'239.129.2.3'};
 assert.equal(E.validateIpMulticast(ipv4,'01:00:5e:01:02:03','2048').group[1],129);
 const ipv6={family:'ipv6',source:'2001:db8::1',group:'ff02::102:304'};
 const expanded={...ipv6,source:'2001:0db8:0:0:0:0:0:1',group:'ff02:0:0:0:0:0:0102:0304'};
 assert.deepEqual(E.validateIpMulticast(ipv6,'33:33:01:02:03:04','34525'),E.validateIpMulticast(expanded,'33:33:01:02:03:04','34525'));
 for(const bad of [{...ipv4,source:'239.1.1.1'},{...ipv4,group:'192.0.2.1'},{...ipv6,group:'ff02:::'},{...ipv6,source:'2001::db8::1'}])assert.throws(()=>E.validateIpMulticast(bad,'33:33:01:02:03:04','34525'),/Ethernet:/);
 assert.throws(()=>E.validateIpMulticast(ipv4,'01:00:5e:01:02:04','2048'),/Ethernet:/);
});
function dynamicFixture(){
 const raw=vlanFixture();raw.metadata.model_profile='ethernet.l2.dynamic.v1';
 raw.metadata.model_schemas=['frame','transfer','reception','control','policy'].map(k=>({schema_name:`ethernet.dynamic.${k}`,schema_version:1}));
 for(const f of raw.metadata.flows)f.ip_multicast=null;
 const ids=new Map([['g:0@N.a.tx','g:0/v0/c1'],['g:0@N.b.tx_c','g:0/v1/c2']]);let effect=1;
 for(const row of raw.simulation.model_records){const kind=row.schema_name.split('.')[1];row.schema_name=`ethernet.dynamic.${kind}`;row.schema_version=1;row.subject='@network';row.data.effect_seq=String(effect++);
  for(const key of ['record_id']){if(ids.has(row[key]))row[key]=ids.get(row[key]);else if(row[key].endsWith('@rx'))row[key]=ids.get(row[key].slice(0,-3))+'@rx';}
  const d=row.data;for(const key of ['parent_transfer_id','transfer_id'])if(ids.has(d[key]))d[key]=ids.get(d[key]);if(d.egress_transfer_ids)d.egress_transfer_ids=d.egress_transfer_ids.map(id=>ids.get(id));
  if(kind==='frame')d.ip_multicast=null;
  if(kind==='transfer')Object.assign(d,{visit_id:d.parent_transfer_id===null?'0':'1',copy_id:row.record_id,policy_epoch_offer:'0',policy_epoch_sof:'0'});
  if(kind==='reception')Object.assign(d,{visit_id:d.transfer_id.endsWith('c1')?'1':'2',policy_epoch_ingress:'0'});
 }
 const ports=raw.metadata.ethernet_topology.ports;
 const snapshot={policy_epoch:'0',topology_generation:'0',roles:Object.fromEntries(ports.map(p=>[p.port,'designated'])),link_up:Object.fromEntries(ports.map(p=>[p.port,true])),effective_vlans:Object.fromEntries(ports.map(p=>[p.port,Object.fromEntries(p.vlans.map(v=>[v.vid,v.tagged]))]))};
 raw.metadata.ethernet_dynamic={initial_policy:snapshot,config:{mac_age_ps:'5000000',convergence_ps:'1000000',bridges:[{instance:'N.b',bridge_id:'1'}],links:[{id:'a-b',ports:['N.a.tx','N.b.tx_a'],up:true,cost:'1'},{id:'b-c',ports:['N.b.tx_c','N.c.tx'],up:true,cost:'1'},{id:'b-quiet',ports:['N.b.tx_quiet','N.quiet.tx'],up:true,cost:'1'}],registrable:[],limits:{mac_entries:32,membership_entries:32,sources_per_entry:32,registrations:32,control_events:32,pending_timers:32,visits_per_frame:32}}};
 raw.simulation.model_records.unshift({schema_name:'ethernet.dynamic.policy',schema_version:1,record_id:'policy/0',subject:'@network',request_id:null,origin_request_id:null,time_ps:'0',data:{time_ps:'0',policy_epoch:'0',topology_generation:'0',initial:true,changes:[{table:'policy',key:null,before:null,after:snapshot}],effect_seq:'0'}});
 return raw;
}
test('dynamic loader supports versioned visit IDs and quiet policy state',()=>{
 const m=E.parseResults(dynamicFixture());assert(m.dynamic);assert.equal(m.transfers.size,2);assert.equal(m.devices.size,4);assert.equal(E.stateAt(m,0n).policy.epoch,0n);assert.equal(E.stateAt(m,0n).policy.snapshot.roles['N.quiet.tx'],'designated');
 const expected=E.stepTransfers(m,609000n);E.stateAt(m,m.end);assert.deepEqual(E.stepTransfers(m,609000n),expected);assert.equal(expected[0].id,'g:0/v0/c1');
});
test('dynamic filtering reasons preserve a completed switch reception without child copies',()=>{
 for(const reason of ['known_egress_ineligible','multicast_filtered','visit_limit']){
  const raw=dynamicFixture();
  raw.simulation.model_records=raw.simulation.model_records.filter(r=>!['g:0/v1/c2','g:0/v1/c2@rx'].includes(r.record_id));
  const row=raw.simulation.model_records.find(r=>r.record_id==='g:0/v0/c1@rx');
  Object.assign(row.data,{status:'filtered',reason,egress_transfer_ids:[]});
  const parsed=E.parseResults(raw);assert.equal(parsed.receptions.get(row.record_id).reason,reason);assert.equal(parsed.transfers.size,1);
 }
});
test('dynamic loader rejects unknown schema, epochs, visit IDs and version mixing',()=>{
 for(const mutate of [r=>r.simulation.model_records[1].schema_name='ethernet.frame',r=>r.simulation.model_records[2].data.policy_epoch_offer='99',r=>r.simulation.model_records[2].data.copy_id='missing',r=>r.simulation.model_records[4].data.visit_id='0',r=>r.simulation.model_records[0].schema_version=2,r=>r.simulation.model_records[0].data.changes[0].after.roles['N.quiet.tx']='forwarding',r=>r.simulation.model_records[1].data.effect_seq='0']){
  const raw=dynamicFixture();mutate(raw);assert.throws(()=>E.parseResults(raw),/Ethernet:/);
 }
});
module.exports.dynamicFixture=dynamicFixture;
function tsnFixture(){
 const raw=dynamicFixture();raw.metadata.model_profile='ethernet.tsn.v1';raw.metadata.model_schemas.push(...['gate','credit','policing','decision'].map(k=>({schema_name:`ethernet.tsn.${k}`,schema_version:1})));
 raw.metadata.ethernet_tsn={clock:'ideal_shared',outputs:raw.metadata.ethernet_topology.directions.map(d=>({port:d.from_port,tas:null,cbs:d.from_port==='N.a.tx'?[{priority:7,idle_slope_bps:'250000000',hi_credit_bits:'1000',lo_credit_bits:'1000'}]:[]})),streams:[],gcl_updates:[]};
 const row=(kind,id,time,data)=>({schema_name:`ethernet.tsn.${kind}`,schema_version:1,record_id:id,subject:'@network',request_id:null,origin_request_id:null,time_ps:String(time),data});
 raw.simulation.model_records.push(row('credit','credit/0',0,{port:'N.a.tx',priority:'7',sign:'positive',magnitude:'0',scale:'1000000000000',slope_bps:'-750000000',cause:'sof',effect_seq:'6'}),row('credit','credit/1',704000,{port:'N.a.tx',priority:'7',sign:'negative',magnitude:'528000000000000',scale:'1000000000000',slope_bps:'250000000',cause:'release',effect_seq:'7'}),row('decision','decision/0',0,{port:'N.a.tx',transfer_id:'g:0/v0/c1',priority:'7',state:'busy',policy_epoch:'0',schedule_generation:'0',next_wake_ps:'704000',reason:'busy',effect_seq:'8'}));
 return raw;
}
test('TSN loader replays signed credit through IFG and shows quiet gate configuration',()=>{
 const m=E.parseResults(tsnFixture());assert(m.tsn);assert.equal(E.stateAt(m,608000n).tsn.credits.get('N.a.tx/7'),-456000000000000n);assert.equal(E.stateAt(m,704000n).tsn.credits.get('N.a.tx/7'),-528000000000000n);assert.equal(E.stateAt(m,1704000n).tsn.credits.get('N.a.tx/7'),-278000000000000n);
 assert.deepEqual(E.stateAt(m,100n).tsn.gates.get('N.quiet.tx').open,[0,1,2,3,4,5,6,7]);const state=E.stateAt(m,1704000n);E.stateAt(m,0n);assert.deepEqual(E.stateAt(m,1704000n),state);
});
test('TSN loader rejects negative zero, unknown states, overflowing numerator and incomplete schema sets',()=>{
 for(const mutate of [r=>r.simulation.model_records[6].data.sign='negative',r=>r.simulation.model_records[6].data.magnitude=String(1n<<128n),r=>r.simulation.model_records[8].data.state='mystery',r=>r.metadata.model_schemas.pop()]){const raw=tsnFixture();mutate(raw);assert.throws(()=>E.parseResults(raw),/Ethernet:/);}
});
module.exports.tsnFixture=tsnFixture;
function compositeFixture(){
 const raw=vlanFixture();raw.metadata.model_profile='can.ethernet.gateway.v1';raw.metadata.model_schemas.unshift({schema_name:'can.request',schema_version:1},{schema_name:'can.receiver',schema_version:1});raw.metadata.model_schemas.push(...['conversion','branch','segment'].map(k=>({schema_name:`dir.can_ethernet.${k}`,schema_version:1})));
 raw.metadata.topology={controllers:[{id:'N.gwcan',bus:'N.canbus',tx_channel_delay_ps:'0',rx_channel_delay_ps:'0'},{id:'N.canSink',bus:'N.canbus',tx_channel_delay_ps:'0',rx_channel_delay_ps:'0'}]};raw.metadata.can_ethernet={gateways:[{instance:'N.gateway',can_ports:['N.gwcan'],ethernet_endpoint:'N.quiet',rx_capacity:2,conversion_delay_ps:'2000000',max_hops:8,rules:[]}]};
 raw.simulation.records=[];
 const envelope=(schema,id,subject,request,time,data)=>({schema_name:schema,schema_version:1,record_id:id,subject,request_id:request,origin_request_id:request,time_ps:time,data});
 raw.simulation.model_records.push(envelope('can.request','can:0','N.canSink','can:0','300',{request_id:'can:0',source:'N.canSink',bus:'N.canbus',status:'success',generated_ps:'0',ready_ps:'0',sof_ps:'100',eof_ps:'200',payload_bits:'0',serialized_bits:'50',attempts:'1',retries:'0',drop_reason:null,model_fields:{profile:'can.cc.multibus.v1',schema_version:1,crc15:'0',stuff_bits:'3',frame_bits:'50',intermission_bits:'3',bitrate_bps:'500000',planned_eof_ps:'200',planned_release_ps:'300',release_ps:'300',origin_request_id:'can:0',parent_request_id:null,gw_hops:'0'}}));
 raw.simulation.model_records.push(envelope('can.receiver','can:0/N.gwcan','N.gwcan','can:0','202',{request_id:'can:0',receiver:'N.gwcan',status:'received',observed_ps:'201',received_ps:'202'}));
 raw.simulation.model_records.push(envelope('dir.can_ethernet.conversion','conversion/no-rule','@network','can:0','201',{conversion_id:'conversion/no-rule',origin_id:'can:0',parent_id:'can:0',gateway:'N.gateway',ingress_record:'can:0/N.gwcan',rule_id:null,visited_gateways:[],observed_ps:'201',ready_ps:null,planned_ready_ps:null,released_ps:null,status:'rejected',reason:'no_rule',branch_ids:[],effect_seq:'1'}));
 raw.simulation.model_records.push({schema_name:'dir.can_ethernet.segment',schema_version:1,record_id:'segment/0',subject:'@network',request_id:'g:0',origin_request_id:'g:0',time_ps:'1198000',data:{segment_id:'segment/0',origin_id:'g:0',source_record_id:'g:0',branch_lineage:[],sof_ps:'0',target_count:'1',targets:[{origin_id:'g:0',segment_id:'segment/0',branch_lineage:[],terminal_id:'N.c',completed_ps:'1198000',completion_reception_id:'g:0@N.b.tx_c@rx'}],effect_seq:'0'}});
 return raw;
}
test('composite loader retains both physical media and frozen SOF terminal counts',()=>{
 const m=E.parseResults(compositeFixture());assert(m.composite);assert.equal(m.canModel.controllers.length,2);assert.equal(E.stateAt(m,100n).composite.can.counts.in_flight,1);assert(E.stepTransfers(m,100n).some(t=>t.medium==='can'&&t.kind==='tx'&&t.from==='N.canSink'&&t.to==='N.canbus'));assert.equal(m.canModel.buses[0],'N.canbus');assert.equal(E.stateAt(m,0n).composite.targetCount,1n);assert.equal(E.stateAt(m,1197999n).composite.completed,0n);assert.equal(E.stateAt(m,1198000n).composite.completed,1n);
 const expected=E.stateAt(m,1198000n);E.stateAt(m,0n);assert.deepEqual(E.stateAt(m,1198000n),expected);
});
test('composite segment loader rejects fabricated completions and Gateway terminal opportunities',()=>{
 for(const mutate of [r=>r.simulation.model_records[8].data.targets[0].completed_ps='1',r=>r.simulation.model_records[8].data.targets[0].terminal_id='N.quiet',r=>r.simulation.model_records[8].data.target_count='2',r=>r.simulation.model_records[8].data.source_record_id='missing']){const raw=compositeFixture();mutate(raw);assert.throws(()=>E.parseResults(raw),/Ethernet:/);}
});
test('DIR codec validates length, ID width, reserved flags and zero padding',()=>{
 assert.deepEqual(E.decodeCanPayload('4449524301000000012302aabb'),{format:'standard',id:291n,length:13n,data:'aabb'});
 assert.equal(E.decodeCanPayload('4449524301011fffffff08ffffffffffffffff'+'00'.repeat(20)).id,536870911n);
 for(const bytes of ['44495243010000000000','4449524301020000000000','4449524301000000080000','4449524301000000000009','444952430100000000000001'])assert.throws(()=>E.decodeCanPayload(bytes),/Ethernet:/);
});
module.exports.compositeFixture=compositeFixture;
test('dynamic policy checkpoints preserve prefix tables, expiry and never reuse a deleted generation',()=>{
 const fixture=dynamicFixture(),initial=fixture.simulation.model_records[0].data,{effect_seq,...first}=initial,key={switch:'N.b',vid:'10',mac:'02:00:00:00:00:01'};
 const options={ports:new Set(fixture.metadata.ethernet_topology.directions.map(d=>d.from_port)),switches:new Set(['N.b']),end:2000n};
 const rows=[first];let before=null;for(let i=1;i<=70;i++){const after={expires_at:'1000',generation:String(i),value:'N.b.tx_a'};rows.push({time_ps:String(i),policy_epoch:String(i),topology_generation:'0',initial:false,changes:[{table:'mac',key,before,after}]});before=after;}
 const history=E.preparePolicyHistory(rows,options);assert.equal(E.policyStateAt(history,63n).tables.mac.values().next().value.generation,'63');assert.equal(E.policyStateAt(history,70n).tables.mac.values().next().value.generation,'70');
 const state=E.policyStateAt(history,70n);state.tables.mac.values().next().value.generation='0';state.snapshot.roles['N.a.tx']='disabled';state.tables.mac.clear();assert.equal(E.policyStateAt(history,70n).tables.mac.values().next().value.generation,'70');assert.equal(E.policyStateAt(history,70n).snapshot.roles['N.a.tx'],'designated');assert.equal(E.policyStateAt(history,70n).tables.mac.size,1);
 rows.push({time_ps:'1000',policy_epoch:'71',topology_generation:'0',initial:false,changes:[{table:'mac',key,before,after:null}]},{time_ps:'1001',policy_epoch:'72',topology_generation:'0',initial:false,changes:[{table:'mac',key,before:null,after:{expires_at:'1500',generation:'71',value:'N.b.tx_c'}}]});
 assert.equal(E.policyStateAt(E.preparePolicyHistory(rows,options),1000n).tables.mac.size,0);rows.at(-1).changes[0].after.generation='1';assert.throws(()=>E.preparePolicyHistory(rows,options),/世代/);
});
test('PSFP records conserve two buckets and distinguish actual from potential refill',()=>{
 const raw=tsnFixture(),stream={id:'s',ingress:'N.b.rx_a',dst_mac:raw.metadata.flows[0].dst_mac,vid:10,priority:7,max_sdu_bytes:'100',gate:null,meter:{committed_rate_bps:'1000000',peak_rate_bps:'2000000',committed_burst_bytes:'128',peak_burst_bytes:'128',yellow_action:'pass'}};raw.metadata.ethernet_tsn.streams=[stream];
 raw.simulation.model_records.push({schema_name:'ethernet.tsn.policing',schema_version:1,record_id:'policing/0',subject:'@network',request_id:'g:0',origin_request_id:null,time_ps:'609000',data:{stream_id:'s',reception_id:'g:0/v0/c1@rx',ingress:'N.b.rx_a',mac_bytes:'68',verdict:'pass',color:'green',committed_before:'1024000000000000',committed_after:'480000000000000',peak_before:'1024000000000000',peak_after:'480000000000000',consumed_bits:'1088',effect_seq:'9'}});
 const m=E.parseResults(raw);assert.equal(E.stateAt(m,608999n).tsn.meters.get('s').committed,1024000000000000n);assert.equal(E.stateAt(m,609000n).tsn.meters.get('s').committed,480000000000000n);const later=E.stateAt(m,1609000n).tsn.meters.get('s');assert.equal(later.committed,480000000000000n);assert.equal(later.potential_committed,481000000000000n);assert.equal(later.potential_peak,482000000000000n);
 for(const change of [d=>d.consumed_bits='544',d=>d.committed_after='1',d=>d.committed_before='0',d=>d.color='yellow']){const broken=structuredClone(raw);change(broken.simulation.model_records.at(-1).data);assert.throws(()=>E.parseResults(broken),/Ethernet:/);}
});
function admittedCompositeFixture(){
 const raw=compositeFixture(),gateway=raw.metadata.can_ethernet.gateways[0];gateway.ethernet_endpoint='N.c';gateway.conversion_delay_ps='200';gateway.rules=[{id:'to-can',direction:'ethernet_to_can',ingress:'N.c',match:{vid:20,pcp:2,format:'standard',can_id:0},egresses:[{port:'N.gwcan.tx',format:'extended',can_id:291}]}];
 function crc(hex){let c=0xffffffff;for(const pair of hex.match(/../g)){c^=parseInt(pair,16);for(let i=0;i<8;i++)c=(c>>>1)^((c&1)?0xedb88320:0);}c=(c^0xffffffff)>>>0;return [0,8,16,24].map(i=>((c>>>i)&255).toString(16).padStart(2,'0')).join('');}
 function codecWire(w){w.ether_type='34997';w.data_hex='4449524301000000000000';w.pad_bytes='35';const header=w.dst_mac.replaceAll(':','')+w.src_mac.replaceAll(':','')+(w.tag?'8100'+((Number(w.tag.pcp)<<13)|(Number(w.tag.dei)<<12)|Number(w.tag.vid)).toString(16).padStart(4,'0'):'')+'88b5';const body=header+w.data_hex+'00'.repeat(35);w.fcs_hex=crc(body);w.mac_hex=body+w.fcs_hex;}
 codecWire(raw.simulation.model_records[0].data);codecWire(raw.simulation.model_records[1].data.wire);codecWire(raw.simulation.model_records[3].data.wire);
 const segment=raw.simulation.model_records[8];segment.time_ps='0';segment.data.target_count='0';segment.data.targets=[];
 const native=structuredClone(raw.simulation.model_records[5]);native.record_id='child-can';native.subject='N.gwcan';native.request_id='child-can';native.origin_request_id='child-can';native.time_ps='1200000';Object.assign(native.data,{request_id:'child-can',source:'N.gwcan',status:'pending',generated_ps:'1200000',ready_ps:'1200000',sof_ps:null,eof_ps:null});Object.assign(native.data.model_fields,{origin_request_id:'child-can',planned_eof_ps:null,planned_release_ps:null,release_ps:null});raw.simulation.model_records.push(native);
 const record=(kind,id,time,data)=>({schema_name:`dir.can_ethernet.${kind}`,schema_version:1,record_id:id,subject:'@network',request_id:'g:0',origin_request_id:'g:0',time_ps:time,data});
 raw.simulation.model_records.push(record('conversion','conversion/to-can','1200000',{conversion_id:'conversion/to-can',origin_id:'g:0',parent_id:'g:0@N.b.tx_c',gateway:'N.gateway',ingress_record:'g:0@N.b.tx_c@rx',rule_id:'to-can',visited_gateways:['N.gateway'],observed_ps:'1197000',ready_ps:'1198200',planned_ready_ps:'1198200',released_ps:'1200000',status:'released',reason:null,branch_ids:['branch/to-can'],effect_seq:'2'}));
 raw.simulation.model_records.push(record('branch','branch/to-can','1200000',{branch_id:'branch/to-can',conversion_id:'conversion/to-can',egress:'N.gwcan.tx',child_id:'child-can',planned_child_id:'child-can',codec_length:'11',pcp:null,input_format:'standard',input_can_id:'0',output_format:'extended',output_can_id:'291',offer_ps:'1200000',planned_offer_ps:'1200000',admitted_ps:'1200000',sof_ps:null,status:'admitted',reason:null,effect_seq:'3'}));
 return raw;
}
test('composite branch closure keeps origin and RX capacity while awaiting a real child',()=>{
 const raw=admittedCompositeFixture(),m=E.parseResults(raw);assert.equal(m.originByRecord.get('child-can'),'g:0');assert.equal(m.origins.has('child-can'),false);assert.equal(E.stateAt(m,1197000n).composite.held,1);assert.equal(E.stateAt(m,1198200n).composite.branches.get('branch/to-can'),'waiting');assert.equal(E.stateAt(m,1200000n).composite.held,0);assert.equal(E.stateAt(m,1200000n).composite.branches.get('branch/to-can'),'admitted');
 for(const change of [d=>d.output_can_id='292',d=>d.child_id='missing',d=>d.codec_length='12',d=>d.admitted_ps='1199999']){const broken=structuredClone(raw);change(broken.simulation.model_records.at(-1).data);assert.throws(()=>E.parseResults(broken),/Ethernet:/);}
});
test('composite origin identity must agree with the recorded ingress media',()=>{const raw=admittedCompositeFixture();raw.simulation.model_records.at(-2).data.origin_id='can:0';assert.throws(()=>E.parseResults(raw),/origin/);});
module.exports.admittedCompositeFixture=admittedCompositeFixture;
test('composite segment IDs share native source IDs while duplicate schema identities reject',()=>{
 const raw=compositeFixture(),segment=raw.simulation.model_records.at(-1);segment.record_id='g:0';segment.data.segment_id='g:0';segment.data.targets[0].segment_id='g:0';raw.simulation.records.push({metric:'queue_length',target:'N.a.tx.queue.7',time_ps:'0',value_kind:'integer',value:'0'});assert.equal(E.parseResults(raw).segments.get('g:0').target_count,'1');raw.simulation.model_records.push(structuredClone(segment));assert.throws(()=>E.parseResults(raw),/schema\/ID/);
});
test('partial mixed replay retains admitted children without future SOF or fabricated receivers',()=>{
 const raw=admittedCompositeFixture();raw.simulation.partial=true;raw.simulation.termination='time_limit';raw.simulation.end_ps='1300000';raw.simulation.last_event_time_ps='1250000';const m=E.parseResults(raw),child=m.canModel.requests.find(r=>r.id==='child-can');assert.equal(child.sof,null);assert.equal(child.receivers.length,0);assert.equal(E.stateAt(m,m.end).composite.targetCount,0n);assert.equal(E.stateAt(m,m.end).composite.completed,0n);assert.equal(E.stateAt(m,m.end).composite.held,0);
});
test('TSN decisions require the visible schedule generation and physical copy class',()=>{
 for(const change of [d=>d.schedule_generation='9',d=>d.port='N.quiet.tx',d=>d.priority='6',d=>d.priority=7]){const raw=tsnFixture();change(raw.simulation.model_records.at(-1).data);assert.throws(()=>E.parseResults(raw),/Ethernet:/);}
});

test('quiet TSN networks show configured gates and initial CBS credit without traffic',()=>{const raw=tsnFixture();raw.simulation.model_records=raw.simulation.model_records.filter(r=>r.schema_name==='ethernet.dynamic.policy');const m=E.parseResults(raw),state=E.stateAt(m,m.end);assert.equal(m.frames.size,0);assert.equal(state.tsn.credits.get('N.a.tx/7'),0n);assert.equal(state.tsn.gates.size,m.directions.size);});

test('dynamic control audit follows time and effect sequence independent of file ordering',()=>{const raw=dynamicFixture();const control=(id,time,effect)=>({schema_name:'ethernet.dynamic.control',schema_version:1,record_id:id,subject:'@network',request_id:null,origin_request_id:null,time_ps:time,data:{control_id:id,kind:'mac_flush',scheduled_ps:time,applied_ps:time,batch_ordinal:'0',epoch_before:'0',epoch_after:'0',generation:null,outcome:'no_op',key:null,before:null,after:null,effect_seq:effect}});raw.simulation.model_records.push(control('late','30','8'),control('same-time-last','20','7'),control('first','20','6'));assert.deepEqual(E.parseResults(raw).controlRows.map(r=>r.record_id),['first','same-time-last','late']);});

test('zero horizon retains only the frozen initial network policy',()=>{for(const fixture of [dynamicFixture,tsnFixture]){const raw=fixture();raw.simulation.end_ps='0';raw.simulation.termination='time_limit';raw.simulation.model_records=raw.simulation.model_records.filter(r=>r.schema_name==='ethernet.dynamic.policy');const m=E.parseResults(raw);assert.equal(m.frames.size,0);assert.equal(E.stateAt(m,0n).policy.epoch,0n);assert.deepEqual(E.stepTransfers(m,0n),[]);}});

test('zero TSN horizon permits only explicit initial gate and empty ready decision',()=>{const raw=tsnFixture();raw.simulation.end_ps='0';raw.simulation.termination='time_limit';raw.simulation.committed_events='0';raw.simulation.model_records=raw.simulation.model_records.filter(r=>r.schema_name==='ethernet.dynamic.policy');const row=(kind,id,data)=>({schema_name:'ethernet.tsn.'+kind,schema_version:1,record_id:id,subject:'@network',request_id:null,origin_request_id:null,time_ps:'0',data});raw.simulation.model_records.push(row('gate','gate/initial',{port:'N.a.tx',schedule_id:null,generation:'0',open_priorities:['0','1','2','3','4','5','6','7'],next_boundary_ps:null,cause:'initial',effect_seq:'1'}),row('decision','decision/initial',{port:'N.a.tx',transfer_id:null,priority:null,state:'ready',policy_epoch:'0',schedule_generation:'0',next_wake_ps:null,reason:'ready',effect_seq:'2'}));assert.equal(E.parseResults(raw).frames.size,0);for(const change of [r=>r.simulation.committed_events='1',r=>r.simulation.model_records.at(-1).data.priority='7',r=>r.simulation.model_records.at(-2).data.cause='boundary']){const broken=structuredClone(raw);change(broken);assert.throws(()=>E.parseResults(broken),/Ethernet:/);}});
function collisionCompositeFixture(){
 const raw=compositeFixture();for(let i=5;i<8;i++)raw.simulation.model_records[i]=JSON.parse(JSON.stringify(raw.simulation.model_records[i]).replaceAll('can:0','g:0'));
 const request=raw.simulation.model_records[5];request.data.generated_ps='100';request.data.ready_ps='100';raw.simulation.model_records[7].data.origin_id='can:g:0';raw.simulation.model_records[7].origin_request_id='can:g:0';
 const segment=raw.simulation.model_records[8];segment.record_id='ethernet:g:0';segment.origin_request_id='ethernet:g:0';segment.data.segment_id='ethernet:g:0';segment.data.origin_id='ethernet:g:0';segment.data.targets[0].origin_id='ethernet:g:0';segment.data.targets[0].segment_id='ethernet:g:0';
 raw.simulation.model_records.push({schema_name:'dir.can_ethernet.segment',schema_version:1,record_id:'can:g:0',subject:'@network',request_id:null,origin_request_id:'can:g:0',time_ps:'100',data:{segment_id:'can:g:0',origin_id:'can:g:0',source_record_id:'g:0',branch_lineage:[],sof_ps:'100',target_count:'0',targets:[],effect_seq:'2'}});return raw;
}
test('equal native IDs remain distinct qualified media origins and preserve generation times',()=>{
 const raw=collisionCompositeFixture(),m=E.parseResults(raw);assert.equal(m.canModel.requests[0].id,'g:0');assert.equal(m.frames.get('g:0').id,'g:0');assert.equal(m.origins.get('can:g:0').time,100n);assert.equal(m.origins.get('ethernet:g:0').time,0n);assert.equal(m.originByRecord.get('can/g:0'),'can:g:0');assert.equal(m.originByRecord.get('ethernet/g:0'),'ethernet:g:0');assert.equal(m.originByRecord.has('g:0'),false);assert.equal(m.segments.get('ethernet:g:0').sourceMedia,'ethernet');assert.equal(m.segments.get('can:g:0').sourceMedia,'can');assert.equal(E.stateAt(m,m.end).composite.completed,1n);
 for(const change of [r=>r.simulation.model_records[7].data.origin_id='g:0',r=>r.simulation.model_records[8].data.origin_id='g:0',r=>r.simulation.model_records[7].data.origin_id='ethernet:g:0',r=>r.simulation.model_records[8].data.source_record_id='missing']){const broken=structuredClone(raw);change(broken);assert.throws(()=>E.parseResults(broken),/Ethernet:/);}
});
module.exports.collisionCompositeFixture=collisionCompositeFixture;

function registrableReferenceFixture(){
 const raw=dynamicFixture();raw.simulation.model_records=raw.simulation.model_records.filter(r=>r.schema_name==='ethernet.dynamic.policy');
 raw.metadata.ethernet_dynamic.config.registrable=[{port:'N.c.tx',vid:30,tagged:true},{port:'N.b.tx_c',vid:30,tagged:true}];
 raw.metadata.ethernet_topology.devices.find(d=>d.id==='N.c').multicast.push({vid:'30',dst_mac:'01:00:5e:01:02:03'});
 const bridge=raw.metadata.ethernet_topology.devices.find(d=>d.id==='N.b');
 bridge.vlan_fdb.push({vid:'30',dst_mac:'02:00:00:00:00:03',egress:'N.b.tx_c'});
 bridge.multicast.push({vid:'30',dst_mac:'01:00:5e:01:02:03',egresses:['N.b.tx_c']});
 return raw;
}
test('declared dynamic tagged capability validates subscription, FDB and multicast without registering runtime membership',()=>{
 for(const profile of ['ethernet.l2.dynamic.v1','ethernet.tsn.v1']){
  const raw=registrableReferenceFixture();if(profile==='ethernet.tsn.v1'){const tsn=tsnFixture();raw.metadata.model_profile=profile;raw.metadata.model_schemas=tsn.metadata.model_schemas;raw.metadata.ethernet_tsn=tsn.metadata.ethernet_tsn;}
  const m=E.parseResults(raw);assert.equal(m.policies.get('N.c.tx').members.has('30'),false);assert.equal(m.policies.get('N.b.tx_c').members.has('30'),false);
  assert.equal(m.devices.get('N.c').groups.has('30/01:00:5e:01:02:03'),true);assert.equal(m.devices.get('N.b').vlanFdb.get('30/02:00:00:00:00:03'),'N.b.tx_c');assert(m.devices.get('N.b').groups.get('30/01:00:5e:01:02:03').has('N.b.tx_c'));
  for(const time of [0n,m.end]){const policy=E.stateAt(m,time).policy.snapshot;assert.equal(Object.hasOwn(policy.effective_vlans['N.c.tx'],'30'),false);assert.equal(Object.hasOwn(policy.effective_vlans['N.b.tx_c'],'30'),false);}
 }
});
test('dynamic reference validation rejects undeclared, foreign, malformed, untagged and duplicate capabilities',()=>{
 for(const mutate of [
  r=>r.metadata.ethernet_dynamic.config.registrable=[],
  r=>r.metadata.ethernet_dynamic.config.registrable[0].port='N.quiet.tx',
  r=>r.metadata.ethernet_dynamic.config.registrable[0].port='N.missing.tx',
  r=>r.metadata.ethernet_dynamic.config.registrable[0].port='N.c.rx',
  r=>r.metadata.ethernet_dynamic.config.registrable[0].tagged=false,
  r=>r.metadata.ethernet_dynamic.config.registrable[0].vid=true,
  r=>r.metadata.ethernet_dynamic.config.registrable[0].vid='30',
  r=>r.metadata.ethernet_dynamic.config.registrable[0].vid=0,
  r=>r.metadata.ethernet_dynamic.config.registrable[0].vid=4095,
  r=>r.metadata.ethernet_dynamic.config.registrable[0].unexpected=true,
  r=>r.metadata.ethernet_dynamic.config.registrable.push({...r.metadata.ethernet_dynamic.config.registrable[0]}),
  r=>r.metadata.ethernet_dynamic.config.registrable.push({port:'N.c.tx',vid:20,tagged:true}),
  r=>r.metadata.ethernet_topology.devices.find(d=>d.id==='N.b').vlan_fdb.at(-1).egress='N.a.tx',
  r=>r.metadata.ethernet_topology.devices.find(d=>d.id==='N.b').multicast.at(-1).egresses=['N.a.tx'],
 ]){const raw=registrableReferenceFixture();mutate(raw);assert.throws(()=>E.parseResults(raw),/Ethernet:/);}
 for(const type of ['vlan_fdb','multicast']){const raw=registrableReferenceFixture();raw.metadata.ethernet_dynamic.config.registrable=raw.metadata.ethernet_dynamic.config.registrable.filter(c=>c.port!=='N.b.tx_c');if(type==='vlan_fdb')raw.metadata.ethernet_topology.devices.find(d=>d.id==='N.b').multicast.pop();else raw.metadata.ethernet_topology.devices.find(d=>d.id==='N.b').vlan_fdb.pop();assert.throws(()=>E.parseResults(raw),type==='vlan_fdb'?/VLAN FDB/:/multicast egress/);}
});
test('static VLAN references cannot borrow a dynamic capability declaration',()=>{
 const raw=vlanFixture();raw.metadata.ethernet_dynamic={config:{registrable:[{port:'N.c.tx',vid:30,tagged:true}]}};raw.metadata.ethernet_topology.devices.find(d=>d.id==='N.c').multicast.push({vid:'30',dst_mac:'01:00:5e:01:02:03'});assert.throws(()=>E.parseResults(raw),/購読VLAN/);
});
function registrableSourceFixture(){
 const raw=dynamicFixture(),frame=raw.simulation.model_records.find(r=>r.schema_name==='ethernet.dynamic.frame'),port=raw.metadata.ethernet_topology.ports.find(p=>p.port==='N.a.tx');
 port.pvid='20';port.vlans=[{vid:'20',tagged:true}];raw.metadata.ethernet_dynamic.config.registrable=[{port:'N.a.tx',vid:10,tagged:true}];
 const initial=raw.metadata.ethernet_dynamic.initial_policy;initial.effective_vlans['N.a.tx']={'20':true};raw.simulation.model_records[0].data.changes[0].after=structuredClone(initial);
 frame.data.ready_ps=null;frame.time_ps='0';raw.simulation.model_records=[raw.simulation.model_records[0],frame];return raw;
}
test('dynamic native source VLAN uses declared tagged permission while PVID and effective membership stay static',()=>{
 const raw=registrableSourceFixture(),m=E.parseResults(raw);assert.equal(m.frames.get('g:0').source_vlan_id,'10');assert.equal(m.policies.get('N.a.tx').pvid,'20');assert.equal(m.policies.get('N.a.tx').members.has('10'),false);assert.equal(Object.hasOwn(E.stateAt(m,0n).policy.snapshot.effective_vlans['N.a.tx'],'10'),false);
 for(const mutate of [r=>r.metadata.ethernet_dynamic.config.registrable=[],r=>r.metadata.ethernet_dynamic.config.registrable[0].port='N.c.tx']){const broken=structuredClone(raw);mutate(broken);assert.throws(()=>E.parseResults(broken),/source VLAN capability/);}
 const badPvid=structuredClone(raw);badPvid.metadata.ethernet_topology.ports.find(p=>p.port==='N.a.tx').pvid='10';assert.throws(()=>E.parseResults(badPvid),/PVID membership/);
});
const capabilityProductPath=process.env.DIR_VIEWER_CAPABILITY_RESULT;
test('current CLI queued-membership-leave result loads declared subscriptions and source VLANs', {skip:!capabilityProductPath},()=>{
 const raw=JSON.parse(require('node:fs').readFileSync(capabilityProductPath,'utf8')),m=E.parseResults(raw);assert(m.dynamic&&m.tsn);assert.equal(m.policies.get('Net.a.tx').members.has('20'),false);assert([...m.frames.values()].some(f=>f.source_vlan_id==='20'));
 assert.equal(Object.hasOwn(raw.metadata.ethernet_dynamic.initial_policy.effective_vlans['Net.a.tx'],'20'),false);assert.equal(Object.hasOwn(E.stateAt(m,0n).policy.snapshot.effective_vlans['Net.a.tx'],'20'),true);assert.equal(Object.hasOwn(E.stateAt(m,100000000n).policy.snapshot.effective_vlans['Net.a.tx'],'20'),false);
});
