'use strict';
// Run from the repository: node --test tests/media_fd_viewer_model.test.cjs
// Cargo is resolved through PATH; CARGO may name another Cargo executable.
// DIR_SIMULATOR_BIN uses an existing executable and skips the Cargo build.
const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {execFileSync}=require('node:child_process');
const E=require('../crates/dir-simulator/src/tool/viewer/assets/ethernet-model.js');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/model.js');
const repository=path.resolve(__dirname,'..'),temporary=fs.mkdtempSync(path.join(os.tmpdir(),'dir-media-fd-viewer-'));
const binary=require('./helpers/simulator_binary.cjs').simulatorBinary(repository);
function result(group,name){const root=path.join(temporary,`${group}-${name}`);if(!fs.existsSync(root))execFileSync(binary,['run','--config',path.join(repository,'docs/verification/fixtures',group,`${name}.ini`),'--output',root]);return JSON.parse(fs.readFileSync(path.join(root,'results.json')));}
test.after(()=>fs.rmSync(temporary,{recursive:true,force:true}));
test('media replay preserves collision, jam, backoff and retry at each destination',()=>{
 const m=E.parseResults(result('ethernet-media','collision')),a=m.transfers.get('a:0@Main.a.tx');
 assert.equal(E.transferStateAt(a,0n),'transmitting');assert.equal(E.transferStateAt(a,100000n),'jamming');assert.equal(E.transferStateAt(a,960000n),'deferred');assert.equal(E.transferStateAt(a,2020000n),'transmitting');assert.equal(E.transferStateAt(a,7780000n),'serialized');
 assert(m.eventTimes.includes(6080000n));assert.equal(E.transferStateAt(m.transfers.get('b:0@Main.b.tx'),6080000n),'deferred');
 assert.equal(m.attempts.size,4);assert.equal(m.physicalLinks.size,1);assert.equal(E.stateAt(m,100000n).counts.jamming,2);
 for(const t of m.eventTimes){const expected=E.stepTransfers(m,t);E.stateAt(m,m.end);E.stateAt(m,0n);assert.deepEqual(E.stepTransfers(m,t),expected);}
 assert(E.stepTransfers(m,100000n).every(t=>t.attempt!=='a:0@Main.a.tx#1'));
});
test('partial jam and T1 arrival boundary retain null actuals and never fabricate reception',()=>{
 const jam=E.parseResults(result('ethernet-media','stop-jam'));
 assert.equal(E.stateAt(jam,jam.end).counts.jamming,2);assert.equal(jam.receptions.size,0);assert([...jam.attempts.values()].every(a=>a.jam_end_ps===null&&a.eof_ps===null));
 const t1=E.parseResults(result('ethernet-media','t1-boundary'));assert.equal(t1.receptions.size,0);assert.equal([...t1.transfers.values()][0].arrival_ps,null);
});
test('T1 pipeline receives an older immutable attempt after a newer SOF',()=>{
 const m=E.parseResults(result('ethernet-media','t1-pipeline'));assert.equal(m.receptions.size,2);assert.equal(m.transfers.get('a:1@Main.a.tx').sof_ps,672000n);assert.equal(m.transfers.get('a:0@Main.a.tx').arrival_ps,2577000n);assert.equal(E.stateAt(m,2577000n).counts.received,1);
});
test('CAN FD replay keeps two-rate timing, evidence and declared wiring',()=>{
 const raw=result('original-network','fd'),m=M.parseResults(raw);assert.equal(m.canfd,true);assert.equal(m.requests[0].eof,380000000n);assert.equal(m.requests[0].release,386000000n);assert.equal(M.stateAt(m,379999999n).counts.in_flight,1);assert.equal(M.stateAt(m,380000000n).counts.success,1);
 assert.equal(m.requests[0].fdFrame.fidelity,'externally-precomputed-phase-bits');assert.equal(m.requests[0].fdFrame.wire_validation,'structural-only');assert.equal(m.controllers.length,3);assert.equal(m.raw,raw);
 for(const mutate of [r=>r.simulation.model_records.find(x=>x.schema_name==='dir.canfd.frame').data.dlc=0,r=>r.simulation.model_records.find(x=>x.schema_name==='dir.canfd.request').data.planned_eof_ps='1',r=>r.simulation.model_records.find(x=>x.schema_name==='dir.canfd.reception').data.receiver='missing']){const broken=structuredClone(raw);mutate(broken);assert.throws(()=>M.parseResults(broken));}
});
test('media decoder rejects fabricated completion, attempt lineage and mixed schemas',()=>{
 const raw=result('ethernet-media','stop-jam');
 for(const mutate of [r=>r.simulation.model_records.find(x=>x.schema_name==='ethernet.attempt').data.eof_ps='1',r=>r.simulation.model_records.find(x=>x.schema_name==='ethernet.attempt').data.transfer_id='missing',r=>r.simulation.model_records.find(x=>x.schema_name==='ethernet.transfer').schema_version=1]){const broken=structuredClone(raw);mutate(broken);assert.throws(()=>E.parseResults(broken),/Ethernet:/);}
});

test('initial carrier deferral remains in the FIFO and rewinds identically',()=>{
 const root=path.join(temporary,'initial-deferral');fs.mkdirSync(root);
 const fixture=path.join(repository,'docs/verification/fixtures/ethernet-media'),workload=JSON.parse(fs.readFileSync(path.join(fixture,'collision.workload.json')));
 workload.generators.find(g=>g.id==='b').times_ps=['1000000'];fs.writeFileSync(path.join(root,'workload.json'),JSON.stringify(workload));
 const ini=fs.readFileSync(path.join(fixture,'collision.ini'),'utf8').replace('"models"',JSON.stringify(path.join(fixture,'models'))).replace('"collision.model.json"',JSON.stringify(path.join(fixture,'collision.model.json'))).replace('"collision.workload.json"','"workload.json"').replace(/sim-time-limit\s*=.*$/m, 'sim-time-limit = 2000000ps');
 fs.writeFileSync(path.join(root,'case.ini'),ini);execFileSync(binary,['run','--config',path.join(root,'case.ini'),'--output',path.join(root,'output')]);
 const m=E.parseResults(JSON.parse(fs.readFileSync(path.join(root,'output/results.json')))),b=m.transfers.get('b:0@Main.b.tx');assert.equal(b.status,'deferred');
 for(const t of [1000000n,1999999n,2000000n]){assert.equal(E.transferStateAt(b,t),'deferred');assert.equal(E.stateAt(m,t).counts.deferred,1);assert.deepEqual(E.stateAt(m,t).queues.get('Main.b.tx'),[b.id]);}
 assert.equal(E.transferStateAt(b,999999n),'not_created');
});
