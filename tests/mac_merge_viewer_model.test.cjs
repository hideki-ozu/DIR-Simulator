'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/controlled-model.js');
function fixture(){
 const row=(schema,id,time,data)=>({schema_name:schema,schema_version:1,record_id:id,subject:'p',time_ps:time,data:{effect_seq:'0',...data}});
 return {schema_version:2,metadata:{model_profile:'ethernet.network.composite.v1',model_schemas:['dir.mac_merge.packet','dir.mac_merge.assembly','dir.mac_merge.verification','ethernet.dynamic.reception'].map(schema_name=>({schema_name,schema_version:1}))},simulation:{end_ps:'8000000',committed_events:'1',model_records:[
 row('dir.mac_merge.packet','p0','672000',{packet_id:'p0',port:'p',kind:'Start',sof_ps:'0',eof_ps:'576000',release_ps:'672000',arrival_ps:'576000',planned_eof_ps:'576000',planned_release_ps:'672000',planned_arrival_ps:'576000',bitrate_bps:'1000000000',wire_hex:'55'.repeat(7)+'e6'+'00'.repeat(60)+'f7761204'}),
 row('dir.mac_merge.assembly','a0','576000',{port:'p',incomplete:true,assembled_octets:'60',expected_fragment:'0',errors:[]}),
 row('ethernet.dynamic.reception','r0','7264000',{observed_ps:'7264000',ready_ps:'7264000',status:'received'})],records:[],summary:[]}};
}
test('MAC packet replay shows emitted bytes and actual milestones, with no early whole-MAC receive',()=>{
 const m=M.parseResults(fixture()),before=M.at(m,300000n),packet=before.events.find(e=>e.kind==='dir.mac_merge.packet');
 assert.equal(packet.data.actual_tx_octets,'37');assert.equal(packet.data.eof_ps,null);assert.equal(packet.data.arrival_ps,null);
 assert.equal(before.events.some(e=>e.kind==='ethernet.dynamic.reception'),false);
 assert.equal(M.at(m,576000n).events.find(e=>e.kind==='dir.mac_merge.packet').data.actual_tx_octets,'72');
 assert.equal(M.at(m,7263999n).events.some(e=>e.kind==='ethernet.dynamic.reception'),false);
 assert.equal(M.at(m,7264000n).events.some(e=>e.kind==='ethernet.dynamic.reception'),true);
 assert.deepEqual(M.at(m,300000n),before);
});
