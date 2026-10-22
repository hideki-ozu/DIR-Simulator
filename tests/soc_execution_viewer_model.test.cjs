'use strict';
const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {execFileSync}=require('node:child_process');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/controlled-model.js');
const root=path.resolve(__dirname,'..'),temporary=fs.mkdtempSync(path.join(os.tmpdir(),'dir-soc-viewer-'));
const binary=require('./helpers/simulator_binary.cjs').simulatorBinary(root);
test.after(()=>fs.rmSync(temporary,{recursive:true,force:true}));
test('actual CPU/coherence/native bus and DDR outputs replay in causal order',()=>{
 for(const scenario of ['soc-execution','ddr','mailbox']){
  const output=path.join(temporary,scenario);
  execFileSync(binary,['run','--config',path.join(root,`examples/soc-execution/${scenario}.ini`),'--output',output]);
  const raw=JSON.parse(fs.readFileSync(path.join(output,'results.json'))),model=M.parseResults(raw);
  assert.equal(raw.schema_version,3);assert.equal(raw.metadata.model_profile,'soc.execution.v1');
  const names=new Set(model.events.map(e=>e.kind));assert(names.has('dir.soc.ExecutionRecord'));assert(names.has('fabric.Record'));if(scenario!=='mailbox')assert(names.has('ahb.pipeline.Record'));assert(names.has('dir.noc.FlitRecord'));
  assert(model.events.some(e=>e.data.kind==='request_completed'));
  if(scenario==='ddr')assert(model.events.some(e=>e.data.kind==='ddr_effect'&&e.data.detail.effect.kind==='write_byte'));
  else if(scenario==='mailbox'){assert(model.events.some(e=>e.data.kind==='mailbox_completed'));assert(model.events.some(e=>e.data.kind==='request_completed'&&e.data.request_id==='observe-dma-copy'&&JSON.stringify(e.data.detail.data)==='[7,8,9,10]'));}
  else assert(model.events.some(e=>e.data.kind==='cpu_retired'));
  for(const e of model.events)assert.match(e.data.effect_seq,/^(0|[1-9][0-9]*)$/);
  for(let i=1;i<model.events.length;i++){const a=model.events[i-1],b=model.events[i];assert(a.time<b.time||a.time===b.time&&BigInt(a.data.effect_seq)<BigInt(b.data.effect_seq),'global SoC effect sequence');}
  const end=M.at(model,model.end);M.at(model,0n);assert.deepEqual(M.at(model,model.end),end);
  for(const at of [0n,model.end/2n,model.end])assert(M.at(model,at).events.every(e=>e.time<=at));
  const html=path.join(temporary,`${scenario}.html`);execFileSync(binary,['view','--input',path.join(output,'results.json'),'--output',html]);assert(fs.readFileSync(html,'utf8').includes('soc.execution.v1'));
 }
});
test('generic replay compares safe numeric and exact decimal sequences and rejects lost precision',()=>{
 const base={schema_version:3,metadata:{model_profile:'fixture.sequence',model_schemas:[{schema_name:'fixture.Record',schema_version:1}]},simulation:{end_ps:'1',committed_events:'1',termination:'events_exhausted',records:[],summary:[],model_records:[]}};
 const row=(id,seq)=>({schema_name:'fixture.Record',schema_version:1,record_id:id,subject:'owner',time_ps:'1',data:{effect_seq:seq}});
 base.simulation.model_records=[row('a',10),row('z','2'),row('c','18446744073709551615')];
 assert.deepEqual(M.parseResults(base).events.map(e=>e.data.effect_seq),['2',10,'18446744073709551615']);
 for(const seq of [9007199254740992,-1,1.5,'01','18446744073709551616']){base.simulation.model_records=[row('bad',seq)];assert.throws(()=>M.parseResults(base),/effect seq/);}
});
