'use strict';
const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {execFileSync}=require('node:child_process');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/model.js');
const root=path.resolve(__dirname,'..'),temporary=fs.mkdtempSync(path.join(os.tmpdir(),'dir-iso-fd-viewer-'));
const binary=require('./helpers/simulator_binary.cjs').simulatorBinary(root);
test.after(()=>fs.rmSync(temporary,{recursive:true,force:true}));
function run(name,config){const output=path.join(temporary,name);execFileSync(binary,['run','--config',config,'--output',output]);return JSON.parse(fs.readFileSync(path.join(output,'results.json')));}

test('actual ISO FD results validate every independent full-wire vector and replay reversible milestones',()=>{
 const vectors=JSON.parse(fs.readFileSync(path.join(root,'crates/dir-simulator/tests/fixtures/canfd_iso_wire_vectors.json')));
 const generators=vectors.map((v,index)=>({id:`g${String(index).padStart(3,'0')}`,kind:'can.fd.iso.explicit.v1',node:'Main.a',times_ps:['0'],frame:{format:v.extended?'extended':'standard',id:v.id,data:Buffer.from(v.data).toString('hex'),brs:v.brs,esi:v.esi}}));
 fs.writeFileSync(path.join(temporary,'workload.json'),JSON.stringify({schema_version:2,generators}));
 fs.writeFileSync(path.join(temporary,'model.json'),JSON.stringify({schema_version:1,profile:'can.fd.iso.v1'}));
 fs.writeFileSync(path.join(temporary,'all.ini'),`[General]\nnetwork = fd.Main\nned-path = "${path.join(root,'examples/canfd/models')}"\nmodel-profile = "can.fd.iso.v1"\nmodel-config = "model.json"\nworkload = "workload.json"\nsim-time-limit = 500ms\nMain.bus.profile = "can.fd.iso.v1"\nMain.bus.nominalBitrate = 333333bps\nMain.bus.dataBitrate = 777777bps\nMain.a.queueCapacity = 256\n`);
 const raw=run('all',path.join(temporary,'all.ini')),model=M.parseResults(raw);
 assert.equal(model.fdFrames.size,128);assert.equal(model.requests.length,128);
 vectors.forEach((v,index)=>{const f=model.fdFrames.get(`g${String(index).padStart(3,'0')}`);assert.equal(f.wire_bits,v.bits);assert.equal(f.crc,v.crc);});
 for(const request of [model.requests[0],model.requests.at(-1)]){
  const at=request.eof;const original=M.stateAt(model,at);M.stateAt(model,model.end);M.stateAt(model,0n);assert.deepEqual(M.stateAt(model,at),original);
  assert.equal(M.requestStateAt(request,at-1n),'in_flight');assert.equal(M.requestStateAt(request,at),'success');
 }
 for(const mutate of [d=>d.wire_bits='1'+d.wire_bits.slice(1),d=>d.crc^=1,d=>d.stuff_count^=1,d=>d.stuff_count_code^=1,d=>d.esi=!d.esi,d=>d.brs_index++,d=>d.crc_delimiter_index++,d=>d.dynamic_stuff_positions.push(0),d=>d.fixed_stuff_positions[0]++,d=>d.nominal_bits='1']){
  const broken=structuredClone(raw);mutate(broken.simulation.model_records.find(r=>r.schema_name==='dir.canfd.frame').data);assert.throws(()=>M.parseResults(broken));
 }
});

test('ISO sample exports a standalone Viewer and preserves incomplete-frame observations',()=>{
 const raw=run('sample',path.join(root,'examples/canfd/iso.ini')),model=M.parseResults(raw);
 assert.equal(model.requests.length,4);assert.equal(model.fdFrames.size,3);
 const html=path.join(temporary,'viewer.html');execFileSync(binary,['view','--input',path.join(temporary,'sample/results.json'),'--output',html]);
 assert(fs.readFileSync(html,'utf8').includes('ISO 11898-1:2015 edition 2'));
 const original=fs.readFileSync(path.join(root,'examples/canfd/iso.ini'),'utf8');
 fs.writeFileSync(path.join(temporary,'partial.ini'),original.replace('"models"',`"${path.join(root,'examples/canfd/models')}"`).replace('"iso-model.json"',`"${path.join(root,'examples/canfd/iso-model.json')}"`).replace('"iso-workload.json"',`"${path.join(root,'examples/canfd/iso-workload.json')}"`).replace('5ms','10us'));
 const partial=M.parseResults(run('partial',path.join(temporary,'partial.ini')));
 assert.equal(partial.requests.filter(r=>r.sof!==null).length,1);assert(partial.requests.every(r=>r.eof===null));
 assert.equal(partial.receivers.length,0);
});
