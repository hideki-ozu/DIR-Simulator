'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/transaction-model.js');
const record=(schema,id,subject,request,time,data)=>({schema_name:schema,schema_version:1,record_id:id,subject,request_id:request,origin_request_id:null,time_ps:String(time),data});
function fixture(offset=0n){
 const generated=offset,grant=offset+10n,complete=offset+40n;
 const request=record('axi.transaction','g:0','N.manager','g:0',complete,{manager:'N.manager',interconnect:'N.bus',target:'N.ram',operation:'read',address:'0',beats:'1',generated_ps:String(generated),eligible_ps:String(grant),status:'completed',grant_ps:String(grant),completed_ps:String(complete),response:'OKAY',read_data:['00112233'],drop_reason:null});
 const hs=record('axi.handshake','g:0:R:0','N.bus','g:0',complete,{channel:'R',beat:'0',valid_since_ps:String(offset+30n),address:null,data_hex:'00112233',wstrb:null,last:true,response:'OKAY'});
 const ram=record('axi.memory','N.ram','N.ram',null,0,{base:'0',size:'4',data_hex:'00112233'});
 return{schema_version:2,metadata:{model_profile:'axi4.transaction.v1',model_schemas:['axi.handshake','axi.memory','axi.transaction'].map(schema_name=>({schema_name,schema_version:1}))},simulation:{start_ps:'0',end_ps:String(offset+50n),partial:false,model_records:[request,hs,ram],records:[],summary:[]}};
}
test('replay uses actual grant and completion, preserving integer picoseconds',()=>{const offset=9007199254740993n,m=M.parseResults(fixture(offset));assert.equal(m.requests.get('g:0').generated,offset);assert.equal(M.stateAt(m,offset-1n).generated,0);assert.equal(M.stateAt(m,offset).pending,1);assert.equal(M.stateAt(m,offset+10n).active,1);assert.equal(M.stateAt(m,offset+40n).completed,1);});
test('a stopped active request remains unfinished despite future planned time',()=>{const raw=fixture();raw.simulation.end_ps='35';raw.simulation.model_records.splice(1,1);const r=raw.simulation.model_records[0];r.time_ps='10';Object.assign(r.data,{status:'active',completed_ps:null,read_data:[]});const m=M.parseResults(raw);assert.equal(M.stateAt(m,m.end).active,1);assert.equal(M.stateAt(m,m.end).completed,0);assert.deepEqual(M.stepTransfers(m,m.end),[]);});
test('step replay depends only on the destination interval and recorded direction',()=>{const m=M.parseResults(fixture()),expected=M.stepTransfers(m,40n);M.stateAt(m,50n);M.stepTransfers(m,50n);assert.deepEqual(M.stepTransfers(m,40n),expected);assert.equal(expected[0].from,'N.ram');assert.equal(expected[0].to,'N.manager');});
test('partial prefixes include a committed completion at H',()=>{const raw=fixture();raw.simulation.partial=true;raw.simulation.end_ps='40';const m=M.parseResults(raw);assert.equal(M.stateAt(m,m.end).completed,1);const normal=structuredClone(raw);normal.simulation.partial=false;assert.throws(()=>M.parseResults(normal),/observation/);});
test('unregistered schemas, duplicate IDs, unknown references and invalid times reject',()=>{for(const change of [r=>r.metadata.model_schemas.pop(),r=>r.simulation.model_records[1].schema_version=2,r=>r.simulation.model_records.push(structuredClone(r.simulation.model_records[0])),r=>r.simulation.model_records[1].request_id='missing',r=>r.simulation.model_records[0].data.completed_ps='9',r=>r.simulation.model_records[0].data.generated_ps='00',r=>r.simulation.model_records[0].data.status='pending']){const raw=fixture();change(raw);assert.throws(()=>M.parseResults(raw),/Transaction:/);}});
test('unknown data fields and malformed byte strings reject',()=>{for(const change of [r=>r.simulation.model_records[0].data.extra=true,r=>r.simulation.model_records[2].data.data_hex='ABC']){const raw=fixture();change(raw);assert.throws(()=>M.parseResults(raw),/Transaction:/);}});
module.exports={fixture};

test('terminal timelines stop at actual completion, including rejection at zero',()=>{
 const request={generated:0n,start:null,completed:0n,updated:0n,state:'rejected'};
 assert.deepEqual(M.timelineSegments(request,10n),[{start:0n,end:0n,state:'rejected',dashed:false}]);
 assert.equal(M.requestStateAt(request,0n),'rejected');
 request.completed=3n;request.updated=3n;
 assert.deepEqual(M.timelineSegments(request,10n),[{start:0n,end:3n,state:'pending',dashed:false},{start:3n,end:3n,state:'rejected',dashed:false}]);
 assert.equal(M.requestStateAt(request,2n),'pending');
});
test('timeline preserves pending, active, completed, dropped and failed boundaries',()=>{
 const r={generated:1n,start:null,completed:null,updated:1n,state:'pending'};
 assert.deepEqual(M.timelineSegments(r,10n),[{start:1n,end:10n,state:'pending',dashed:false}]);
 r.start=2n;r.state='active';
 assert.deepEqual(M.timelineSegments(r,10n),[{start:1n,end:2n,state:'pending',dashed:false},{start:2n,end:10n,state:'active',dashed:true}]);
 r.completed=5n;r.state='completed';assert.deepEqual(M.timelineSegments(r,10n).at(-1),{start:2n,end:5n,state:'active',dashed:false});
 r.state='failed';assert.deepEqual(M.timelineSegments(r,10n).at(-1),{start:5n,end:5n,state:'failed',dashed:false});
 r.start=null;r.completed=null;r.updated=1n;r.state='dropped';assert.deepEqual(M.timelineSegments(r,10n),[{start:1n,end:1n,state:'dropped',dashed:false}]);
});
