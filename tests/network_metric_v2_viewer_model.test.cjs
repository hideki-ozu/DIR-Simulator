'use strict';
const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {execFileSync}=require('node:child_process');
const M=require('../crates/dir-simulator/src/tool/viewer/assets/controlled-model.js');
const root=path.resolve(__dirname,'..'),temporary=fs.mkdtempSync(path.join(os.tmpdir(),'dir-network-metric-v2-'));
const binary=require('./helpers/simulator_binary.cjs').simulatorBinary(root);
test.after(()=>fs.rmSync(temporary,{recursive:true,force:true}));
test('actual schema3 network result declares owned gauges and renders CSV/Viewer consistently',()=>{
 const output=path.join(temporary,'run');execFileSync(binary,['run','--config',path.join(root,'examples/ethernet/tsn/tas-metrics.ini'),'--output',output]);
 const raw=JSON.parse(fs.readFileSync(path.join(output,'results.json'))),model=M.parseResults(raw);
 assert.equal(raw.schema_version,3);assert.equal(raw.metadata.model_profile,'ethernet.tsn.v1');
 assert.equal(raw.metadata.metrics_v2.length,3);
 for(const descriptor of raw.metadata.metrics_v2){assert(descriptor.targets.length);assert.equal(new Set(descriptor.targets).size,descriptor.targets.length);if(descriptor.sampling==='gauge')assert.deepEqual(Object.keys(descriptor.initial_gauges),descriptor.targets);}
 const records=raw.simulation.records.filter(r=>r.metric.startsWith('network.'));assert(records.length);
 const summary=raw.simulation.summary.filter(r=>r.metric.startsWith('network.'));assert(summary.length);
 const csv=fs.readFileSync(path.join(output,'summary.csv'),'utf8').trim().split(/\r?\n/).map(line=>line.split(',')),header=csv.shift();
 const at=name=>header.indexOf(name);assert(at('metric')>=0&&at('value')>=0);
 for(const row of summary){const match=csv.find(v=>v[at('metric')]===row.metric&&v[at('target')]===row.target&&v[at('start_ps')]===(row.start_ps??'')&&v[at('end_ps')]===(row.end_ps??''));assert(match,`${row.metric}/${row.target}`);if(row.value_kind==='number'&&row.value!==null)assert.equal(Number(match[at('value')]),row.value);else assert.equal(match[at('value')],row.value===null?'':String(row.value));}
 const atEnd=M.at(model,model.end);M.at(model,0n);assert.deepEqual(M.at(model,model.end),atEnd);
 const viewer=path.join(temporary,'viewer.html');execFileSync(binary,['view','--input',path.join(output,'results.json'),'--output',viewer]);assert(fs.readFileSync(viewer,'utf8').includes('network.queue.frames'));
});
