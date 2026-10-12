'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const crypto = require('node:crypto');
const {spawn} = require('node:child_process');
const {once} = require('node:events');
const binary = process.env.DIR_NED_RUN_TEST_BINARY;
const repo = path.resolve(__dirname,'..');
const delay = ms => new Promise(resolve=>setTimeout(resolve,ms));
const digest = bytes => crypto.createHash('sha256').update(bytes).digest('hex');

for (const signal of ['SIGINT','SIGTERM']) test(`editor ${signal} stops and reaps an active worker`, {
  skip: binary ? false : 'Set DIR_NED_RUN_TEST_BINARY to the freshly built binary', timeout:60000,
}, async ()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'dir-ned-run-shutdown-'));
  const input=path.join(root,'input'), state=path.join(root,'state'), exports=path.join(root,'exports');
  fs.mkdirSync(path.join(input,'models/demo'),{recursive:true});fs.mkdirSync(exports);
  const config=fs.readFileSync(path.join(repo,'examples/can/baseline.ini'),'utf8')
    .replace('20ms','0ps')+'\nmax-events = 1000000000000\n';
  const workload=JSON.parse(fs.readFileSync(path.join(repo,'examples/can/baseline.json'),'utf8'));
  for(const generator of workload.generators) generator.count=1000000000000;
  fs.writeFileSync(path.join(input,'project.ini'),config);
  fs.writeFileSync(path.join(input,'baseline.json'),JSON.stringify(workload));
  fs.copyFileSync(path.join(repo,'examples/can/models/demo/Main.ned'),path.join(input,'models/demo/Main.ned'));
  const sources=['project.ini','baseline.json','models/demo/Main.ned'];
  const before=Object.fromEntries(sources.map(name=>[name,digest(fs.readFileSync(path.join(input,name)))]));
  const editor=spawn(path.resolve(binary),['ned-editor','--config',path.join(input,'project.ini'),'--export-root',exports,'--state-root',state],{cwd:root,stdio:['ignore','pipe','pipe']});
  const closed=once(editor,'close');let workerPid;
  try {
    const startup=await new Promise((resolve,reject)=>{
      let stdout='';const timer=setTimeout(()=>reject(new Error('editor startup timed out')),20000);
      editor.once('error',()=>{clearTimeout(timer);reject(new Error('editor could not start'));});
      editor.stdout.on('data',chunk=>{stdout+=chunk;const end=stdout.indexOf('\n');if(end>=0){clearTimeout(timer);resolve(JSON.parse(stdout.slice(0,end)));}});
    });
    const url=new URL(startup.url), session=new URLSearchParams(url.hash.slice(1)).get('session');
    async function request(route,body) {
      const response=await fetch(url.origin+route,{method:body ? 'POST':'GET',headers:{'X-Editor-Session':session,...(body?{'Content-Type':'application/json','Origin':url.origin}:{})},body:body?JSON.stringify(body):undefined});
      const value=await response.json();assert.ok(response.ok,`HTTP ${response.status}: ${value.code || 'unknown'}`);return value;
    }
    const writer=await request('/api/writer',{action:'register'}), view=await request('/api/session');
    let response=await request('/api/commands',{schema_version:1,session_id:view.session_id,client_id:writer.client_id,writer_epoch:writer.writer_epoch,client_sequence:'1',command_id:writer.client_id+':1',base_revision:view.revision,kind:'start_run',payload:{}});
    const deadline=Date.now()+20000;
    while(!response.terminal){assert.ok(Date.now()<deadline,'freeze job timed out');await delay(40);response=await request('/api/jobs/'+response.job_id);}
    assert.equal(response.operation_status,200);
    const id=response.result.run_id, recordPath=path.join(state,'runs',id,'control/record.json');
    let record;
    do { record=JSON.parse(fs.readFileSync(recordPath,'utf8'));if(record.phase==='running')break;assert.ok(Date.now()<deadline,'worker did not enter running');await delay(40); } while(true);
    workerPid=Number(record.worker_pid);assert.ok(workerPid>0);
    // A stopped child cannot handle TERM, exercising the two-second KILL path.
    if(signal==='SIGINT')process.kill(workerPid,'SIGSTOP');
    editor.kill(signal);await Promise.race([closed,delay(10000).then(()=>{throw new Error('editor shutdown did not complete');})]);
    const final=JSON.parse(fs.readFileSync(recordPath,'utf8'));
    assert.equal(final.phase,'interrupted');assert.equal(final.stop_reason,'service_shutdown');
    assert.equal(final.worker_exit.reaped,true);
    if(signal==='SIGINT')assert.equal(final.worker_exit.signal,9);
    assert.throws(()=>process.kill(workerPid,0),{code:'ESRCH'});
    assert.deepEqual(Object.fromEntries(sources.map(name=>[name,digest(fs.readFileSync(path.join(input,name)))])),before);
    fs.writeFileSync(path.join(root,'evidence.json'),JSON.stringify({signal,binary_sha256:digest(fs.readFileSync(binary)),run_id:id,worker_exit:final.worker_exit,stop_reason:final.stop_reason,input_hashes:before},null,2));
    console.log('Run shutdown evidence: '+path.join(root,'evidence.json'));
  } finally {
    if(editor.exitCode===null && editor.signalCode===null){editor.kill('SIGKILL');await closed;}
    if(workerPid){try{process.kill(workerPid,'SIGKILL');}catch(error){if(error.code!=='ESRCH')throw error;}}
  }
});
