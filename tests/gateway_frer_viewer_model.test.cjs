'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/controlled-model.js');

test('Gateway terminal indications appear at actual suffix reception, including later recovery rounds',()=>{
 const indication=(round,received)=>({schema_name:'dir.can_ethernet.terminal_indication',schema_version:1,record_id:`rx${round}`,subject:'@network',time_ps:received,data:{effect_seq:String(round),origin_id:'can:source:0',source_frame_id:'child',terminal_id:'Net.d',reception_id:`rx${round}`,generated_ps:'0',received_ps:received,end_to_end_ps:received,recovery_round:String(round)}});
 const raw={schema_version:2,metadata:{model_profile:'can.ethernet.tsn.v1',model_schemas:[{schema_name:'dir.can_ethernet.terminal_indication',schema_version:1}]},simulation:{end_ps:'1000',committed_events:'2',records:[],summary:[],model_records:[indication(0,'500'),indication(1,'900')]}};
 const model=M.parseResults(raw),before=M.at(model,499n);
 assert.equal(before.events.length,0);
 assert.deepEqual(M.at(model,500n).events.map(e=>e.data.reception_id),['rx0']);
 assert.deepEqual(M.at(model,899n).events.map(e=>e.data.reception_id),['rx0']);
 assert.deepEqual(M.at(model,900n).events.map(e=>e.data.reception_id),['rx0','rx1']);
 assert.deepEqual(M.at(model,499n),before);
});
