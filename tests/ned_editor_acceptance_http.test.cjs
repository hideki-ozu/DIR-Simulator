'use strict';
// Native HTTP boundaries against a pinned product executable. Credentials and
// Viewer capability paths remain in memory and never enter persisted evidence.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const net = require('node:net');
const crypto = require('node:crypto');
const {spawn} = require('node:child_process');
const {once} = require('node:events');
const repo = path.resolve(__dirname, '..');
const binary = process.env.DIR_NED_HTTP_BINARY;
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const hidden = [];
const redact = text => hidden.reduce((s, secret) => secret ? s.split(secret).join('[redacted]') : s, String(text))
  .replace(/(http:\/\/127\.0\.0\.1:\d+)\/[a-f0-9]{64}/gi, '$1/[redacted]')
  .replace(/#session=[^\s"']+/gi, '#session=[redacted]');
function inventory(root) {
  const result = {};
  function visit(current) {
    for (const name of fs.readdirSync(current).sort()) {
      const target = path.join(current, name), stat = fs.lstatSync(target);
      const key = path.relative(root, target);
      if (stat.isSymbolicLink()) result[key] = {symlink: fs.readlinkSync(target), dev: stat.dev, ino: stat.ino};
      else if (stat.isDirectory()) { result[key] = {directory: true, dev: stat.dev, ino: stat.ino}; visit(target); }
      else result[key] = {bytes: stat.size, sha256: hash(fs.readFileSync(target)), dev: stat.dev, ino: stat.ino, nlink: stat.nlink};
    }
  }
  visit(root); return result;
}
function contents(root) {
  const result = new Map();
  function visit(current) {
    for (const name of fs.readdirSync(current)) {
      const file = path.join(current, name);
      if (fs.lstatSync(file).isDirectory()) visit(file);
      else result.set(path.relative(root, file), fs.readFileSync(file));
    }
  }
  visit(root); return result;
}
function writeContents(root, files) {
  fs.rmSync(root, {recursive: true, force: true}); fs.mkdirSync(root);
  for (const [name, bytes] of files) { const file = path.join(root, name); fs.mkdirSync(path.dirname(file), {recursive: true}); fs.writeFileSync(file, bytes); }
}
function modelState(view) {
  return Object.fromEntries(['revision','input_revision','project_instance_id','snapshot_id','input_digest','dirty','can_undo','can_redo','files','source','types','instances','graph'].map(k => [k, view[k]]));
}
async function raw(origin, target, headers, method = 'GET', body = '') {
  const url = new URL(origin);
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({host: '127.0.0.1', port: Number(url.port)});
    let response = Buffer.alloc(0);
    socket.setTimeout(15000, () => socket.destroy(new Error('raw HTTP timeout')));
    socket.on('error', () => reject(new Error('raw HTTP connection failed')));
    socket.on('data', data => { response = Buffer.concat([response, data]); });
    socket.on('end', () => {
      const text = response.toString('utf8'), first = text.indexOf('\r\n\r\n');
      const status = Number(/^HTTP\/\d\.\d (\d+)/.exec(text)?.[1]);
      const lines = text.slice(0, first).split('\r\n').slice(1);
      resolve({status, headers: lines, body: text.slice(first + 4)});
    });
    socket.on('connect', () => socket.end(`${method} ${target} HTTP/1.1\r\n${headers.map(h => h.join(': ')).join('\r\n')}\r\nConnection: close\r\n${body ? `Content-Length: ${Buffer.byteLength(body)}\r\n` : ''}\r\n${body}`));
  });
}
class NativeEditor {
  constructor(root) { this.root = root; this.input = path.join(root, 'input'); this.state = path.join(root, 'state'); this.exports = path.join(root, 'exports'); this.next = 1; }
  async start() {
    this.child = spawn(path.resolve(binary), ['ned-editor','--config',path.join(this.input,'project.ini'),'--export-root',this.exports,'--state-root',this.state], {cwd:this.root, stdio:['ignore','pipe','pipe']});
    this.closed = once(this.child,'close'); this.stderr = '';
    this.child.stderr.on('data', bytes => { this.stderr = (this.stderr + bytes).slice(-8192); });
    const startup = await new Promise((resolve,reject) => {
      let out = ''; const timer = setTimeout(() => reject(new Error('native editor startup timeout')),20000);
      this.child.once('error', () => {clearTimeout(timer);reject(new Error('native editor launch failed'));});
      this.child.once('close', () => {clearTimeout(timer);reject(new Error('native editor stopped before startup'));});
      this.child.stdout.on('data', bytes => { out += bytes; const end = out.indexOf('\n'); if(end >= 0) {clearTimeout(timer);try{resolve(JSON.parse(out.slice(0,end)));}catch{reject(new Error('invalid startup frame'));}} });
    });
    const url = new URL(startup.url); this.origin = url.origin; this.secret = new URLSearchParams(url.hash.slice(1)).get('session'); hidden.push(this.secret);
    const writer = await this.api('/api/writer',{action:'register'});
    assert.equal(writer.status,200,'writer registration'); this.writer = writer.value; this.next = Number(writer.value.next_sequence);
  }
  async stop() {
    if (this.child && this.child.exitCode === null && this.child.signalCode === null) {
      this.child.kill('SIGTERM');
      const timeout = setTimeout(() => this.child.kill('SIGKILL'),10000);
      await this.closed; clearTimeout(timeout);
    }
  }
  async api(route, body, headers = {}) {
    const response = await fetch(this.origin + route,{method:body === undefined ? 'GET':'POST',headers:{'X-Editor-Session':this.secret,...(body === undefined ? {} : {'Content-Type':'application/json',Origin:this.origin}),...headers},body:body === undefined ? undefined : JSON.stringify(body),signal:AbortSignal.timeout(20000)});
    const text = await response.text(); let value; try {value=JSON.parse(text);}catch{value={};}
    return {status:response.status,value,headers:response.headers,text};
  }
  async view() { const r=await this.api('/api/session'); assert.equal(r.status,200,'session view'); return r.value; }
  async request(kind,payload,sequence=this.next) {const v=await this.view();return {schema_version:1,session_id:v.session_id,client_id:this.writer.client_id,writer_epoch:this.writer.writer_epoch,client_sequence:String(sequence),command_id:this.writer.client_id+':'+sequence,base_revision:v.revision,kind,payload};}
  async terminal(response) {
    const end=Date.now()+30000; let value=response.value;
    while(!value.terminal) {assert.ok(Date.now()<end,'command terminal timeout');assert.ok(value.job_id,'accepted command has job ID');await delay(15);value=(await this.api('/api/jobs/'+value.job_id)).value;}
    if(value.sequence_consumed) this.next=Number(value.next_sequence);
    return value;
  }
  async command(kind,payload) { return this.terminal(await this.api('/api/commands',await this.request(kind,payload))); }
  async runs() { const r=await this.api('/api/runs');assert.equal(r.status,200,'run listing');return r.value.runs; }
  async run(id) {const r=await this.api('/api/runs/'+id);assert.equal(r.status,200,'run detail');return r.value;}
  async finished(id) {const end=Date.now()+60000;for(;;){const r=await this.run(id);if(!['queued','preparing','running','verifying'].includes(r.phase))return r;assert.ok(Date.now()<end,'simulation completion timeout');await delay(25);}}
}
function noCors(response, label) {
  const headers = response.headers instanceof Headers ? [...response.headers].map(([k,v])=>k+': '+v) : response.headers;
  assert.ok(headers.every(h=>!/^access-control-allow-/i.test(h)),label+' must not authorize CORS');
}

// One native fixture keeps the original authoritative run ledger throughout all
// live-access substitutions. No persisted capability or startup secret is needed.
test('native HTTP acceptance XC04 XC32 XC36 XC38 XC39', {skip:binary?false:'Set DIR_NED_HTTP_BINARY to a built product executable',timeout:300000}, async t => {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'ned-native-http-'));
  const editor=new NativeEditor(root);
  fs.mkdirSync(path.join(editor.input,'models/demo'),{recursive:true});fs.mkdirSync(editor.exports);
  for(const [source,dest] of [['examples/can/baseline.ini','project.ini'],['examples/can/baseline.json','baseline.json'],['examples/can/models/demo/Main.ned','models/demo/Main.ned']])fs.copyFileSync(path.join(repo,source),path.join(editor.input,dest));
  // A larger valid input widens the actual asynchronous freeze interval without
  // modifying the parser, protocol, worker, or simulation workload.
  fs.appendFileSync(path.join(editor.input,'models/demo/Main.ned'),'\n// '+ 'freeze input '.repeat(160000)+'\n');
  const external=path.join(root,'external'), recovery=path.join(editor.state,'save-recovery-sentinel');
  fs.mkdirSync(external);fs.mkdirSync(recovery,{recursive:true});
  fs.writeFileSync(path.join(external,'sentinel.bin'),'EXTERNAL-SENTINEL-DO-NOT-EXPORT');fs.writeFileSync(path.join(recovery,'manifest.keep'),'SAVE-RECOVERY-SENTINEL');
  const inputBefore=inventory(editor.input);
  const evidence={schema_version:1,binary_sha256:hash(fs.readFileSync(binary)),node:process.version,platform:process.platform,arch:process.arch,root,subcases:[],gaps:[]};
  const add=(name,data={})=>evidence.subcases.push({name,...data});
  let runA,runB,oldViewer;
  try {
    await editor.start();
    await t.test('XC04 native invalid creation previews are atomic terminal 422',async()=>{
      fs.symlinkSync(external,path.join(editor.input,'models/demo/link'),'dir');
      const beforeModel=modelState(await editor.view()), beforeDisk=inventory(editor.input), beforeExternal=inventory(external);
      for(const [name,relative] of [['empty_package','NoPackage.ned'],['absolute',path.join(external,'Escape.ned')],['traversal','../Escape.ned'],['casefold','demo/main.ned'],['existing_role','demo/Main.ned']]) {
        const result=await editor.command('preview_edit',{operation:'create_file',operation_payload:{role:'ned',root_id:'ned-root-0',relative_path:relative,template_id:'ned.module_empty',declaration:{kind:'module',local_name:'Candidate'},format:{newline:'LF',bom:false}}});
        assert.equal(result.operation_status,422,name+' terminal status');
        assert.deepEqual(modelState(await editor.view()),beforeModel,name+' model atomicity');assert.deepEqual(inventory(editor.input),beforeDisk,name+' disk atomicity');assert.deepEqual(inventory(external),beforeExternal,name+' external atomicity');
        add('xc04_'+name,{operation_status:result.operation_status,error_code:result.code,model_unchanged:true,input_unchanged:true});
      }
      const symlinkCapture=await editor.command('reload',{});assert.equal(symlinkCapture.operation_status,422,'symlink capture rejects Reload');assert.deepEqual(modelState(await editor.view()),beforeModel,'failed symlink capture retains previous model');assert.deepEqual(inventory(editor.input),beforeDisk);assert.deepEqual(inventory(external),beforeExternal);add('xc04_symlink_capture_reload',{operation_status:422,model_unchanged:true,input_unchanged:true});
      fs.unlinkSync(path.join(editor.input,'models/demo/link'));
    });
    await t.test('XC32 discarded accepted HTTP response plus 100 exact resends creates one run',async()=>{
      const request=await editor.request('start_run',{}), body=JSON.stringify(request), url=new URL(editor.origin);
      const socket=net.createConnection({host:'127.0.0.1',port:Number(url.port)});socket.on('error',()=>{});
      await once(socket,'connect');socket.pause();
      socket.write(`POST /api/commands HTTP/1.1\r\nHost: ${url.host}\r\nOrigin: ${editor.origin}\r\nX-Editor-Session: ${editor.secret}\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`);
      // Independent HTTP reads acknowledge server acceptance; this client never
      // reads or decodes the original command response before discarding it.
      let busySeen=false;const deadline=Date.now()+30000;
      while(!(await editor.runs()).length){const view=await editor.view();busySeen ||= view.busy==='start_run';assert.ok(Date.now()<deadline,'lost-response command acceptance');await delay(5);}
      socket.destroy();
      let result;for(let n=0;n<100;n++){result=await editor.terminal(await editor.api('/api/commands',request));assert.equal(result.operation_status,200,'identical resend terminal');runA ||= result.result.run_id;assert.equal(result.result.run_id,runA,'same durable run');}
      const altered=structuredClone(request);altered.payload={wall_timeout_ms:'2000'};
      const conflict=await editor.api('/api/commands',altered);assert.equal(conflict.status,409,'same ID different payload');assert.equal(conflict.value.sequence_consumed,false);
      const runs=await editor.runs();assert.equal(runs.length,1,'one retained run after 100 resends');assert.equal(runs[0].run_id,runA);
      assert.equal((await editor.finished(runA)).phase,'completed');
      add('xc32_response_discard_then_100_resends',{run_id:runA,resends:100,run_count:1,different_payload_status:conflict.status,asynchronous_freeze_observed:busySeen});
      evidence.gaps.push('One retained native execution run is measured; exact worker-launch count and a deterministic in-flight next-sequence barrier require separate instrumented evidence.');
      const next=await editor.command('start_run',{});assert.equal(next.operation_status,200);runB=next.result.run_id;assert.notEqual(runA,runB);assert.equal((await editor.finished(runB)).phase,'completed');
    });
    assert.ok(runA&&runB,'valid A/B runs required for remaining boundaries');
    await t.test('XC39 native raw HTTP Host Origin CORS auth and Viewer authority boundaries',async()=>{
      const built=await editor.command('build_viewer',{run_id:runB});assert.equal(built.operation_status,200,'build valid Viewer');
      oldViewer=new URL(built.result.run.url);hidden.push(oldViewer.pathname.slice(1),oldViewer.href);
      const editorHost=new URL(editor.origin).host, viewerHost=oldViewer.host;
      const editorBefore=modelState(await editor.view());
      for(const target of ['/api/writer','/api/commands']){const denied=await raw(editor.origin,target,[['Host',editorHost],['Origin','http://foreign.invalid'],['X-Editor-Session',editor.secret],['Content-Type','application/json']],'POST','{}');assert.equal(denied.status,403);noCors(denied,'foreign Origin mutation');add('xc39_foreign_origin_mutation',{route:target,status:denied.status,cors:false});}
      const cases=[['bad_host',[['Host','foreign.invalid']]],['malformed_host',[['Host','127.0.0.1:not-a-port']]],['multiple_host',[['Host',editorHost],['Host',editorHost]]],['foreign_origin',[['Host',editorHost],['Origin','http://foreign.invalid']]],['duplicate_origin',[['Host',editorHost],['Origin',editor.origin],['Origin',editor.origin]]]];
      for(const [name,headers] of cases){const r=await raw(editor.origin,'/api/session',[...headers,['X-Editor-Session',editor.secret]]);assert.ok([400,403].includes(r.status),name);noCors(r,name);add('xc39_editor_'+name,{status:r.status,cors:false});}
      const unauth=await raw(editor.origin,'/api/session',[['Host',editorHost]]);assert.equal(unauth.status,403);noCors(unauth,'no auth');
      const options=await raw(editor.origin,'/api/writer',[['Host',editorHost],['Origin','http://foreign.invalid'],['Access-Control-Request-Method','POST']],'OPTIONS');assert.ok([403,405].includes(options.status));noCors(options,'preflight');add('xc39_editor_options',{status:options.status,cors:false});
      for(const [name,headers,method] of [['bad_host',[['Host','foreign.invalid']],'GET'],['multiple_host',[['Host',viewerHost],['Host',viewerHost]],'GET'],['foreign_origin',[['Host',viewerHost],['Origin',editor.origin]],'GET'],['duplicate_origin',[['Host',viewerHost],['Origin',oldViewer.origin],['Origin',oldViewer.origin]],'GET'],['options',[['Host',viewerHost]],'OPTIONS']]){
        const r=await raw(oldViewer.origin,oldViewer.pathname,headers,method);assert.ok([400,403,405].includes(r.status),name+' Viewer gate');noCors(r,name);add('xc39_viewer_'+name,{status:r.status,cors:false});
      }
      const editorSecretOnly=await raw(oldViewer.origin,'/'+editor.secret,[['Host',viewerHost],['X-Editor-Session',editor.secret]]);assert.equal(editorSecretOnly.status,404);noCors(editorSecretOnly,'editor secret on Viewer');
      const wrongCredential=await editor.api('/api/writer',{action:'register'},{'X-Editor-Session':oldViewer.pathname.slice(1)});assert.equal(wrongCredential.status,403);noCors(wrongCredential,'Viewer credential on editor');
      assert.deepEqual(modelState(await editor.view()),editorBefore,'denied requests cannot mutate editor');
      const valid=await fetch(oldViewer.href,{signal:AbortSignal.timeout(20000)});assert.equal(valid.status,200);noCors({headers:valid.headers},'valid Viewer');await valid.arrayBuffer();
      add('xc39_authority_separation',{editor_secret_viewer_status:editorSecretOnly.status,viewer_cap_editor_status:wrongCredential.status,model_unchanged:true,separate_origin:oldViewer.origin!==editor.origin});
      await editor.command('revoke_viewer',{run_id:runB});const revoked=await fetch(oldViewer.href);assert.equal(revoked.status,410);await revoked.arrayBuffer();add('xc39_revoke',{status:410});
      const rebuilt=await editor.command('build_viewer',{run_id:runB});assert.equal(rebuilt.operation_status,200);oldViewer=new URL(rebuilt.result.run.url);hidden.push(oldViewer.pathname.slice(1),oldViewer.href);
      await editor.stop();await editor.start();
      let oldStatus;try{const stale=await fetch(oldViewer.href,{signal:AbortSignal.timeout(5000)});oldStatus=stale.status;await stale.arrayBuffer();assert.ok([404,410].includes(stale.status),'restart old capability denied');}catch(error){if(error.name==='AssertionError')throw error;assert.ok(['ECONNREFUSED','UND_ERR_SOCKET'].includes(error.cause?.code),'old capability listener closed');oldStatus=error.cause.code;}
      const fresh=await editor.command('build_viewer',{run_id:runB});assert.equal(fresh.operation_status,200);const freshURL=new URL(fresh.result.run.url);hidden.push(freshURL.pathname.slice(1),freshURL.href);assert.ok(freshURL.href!==oldViewer.href,'new native process new capability');const page=await fetch(freshURL.href);assert.equal(page.status,200);await page.arrayBuffer();await editor.command('revoke_viewer',{run_id:runB});
      add('xc39_restart',{old_capability:oldStatus,fresh_authenticated_build_status:200});
    });
    await t.test('XC36 native unsafe run cleanup refuses and preserves unrelated identities',async()=>{
      const runDir=path.join(editor.state,'runs',runA), resultDir=path.join(runDir,'result');
      const untouched={input:inventory(editor.input),other_run:inventory(path.join(editor.state,'runs',runB)),recovery:inventory(recovery),external:inventory(external)};
      const resultFile=path.join(resultDir,'results.json'), original=fs.readFileSync(resultFile);
      for(const variant of ['root_unknown_entry','nonempty_unknown_output','symlink','external_hardlink']){
        if(variant==='root_unknown_entry')fs.writeFileSync(path.join(runDir,'unknown.keep'),'UNKNOWN-RUN-ENTRY');
        else if(variant==='nonempty_unknown_output')fs.writeFileSync(path.join(resultDir,'unknown.keep'),'UNKNOWN-OUTPUT');
        else {fs.unlinkSync(resultFile);if(variant==='symlink')fs.symlinkSync(path.join(external,'sentinel.bin'),resultFile);else fs.linkSync(path.join(external,'sentinel.bin'),resultFile);}
        const protectedBefore=inventory(runDir);
        const rejected=await editor.command('delete_run',{run_id:runA});assert.equal(rejected.operation_status,422,variant+' cleanup refusal');assert.deepEqual(inventory(runDir),protectedBefore,variant+' no partial cleanup');
        if(variant==='root_unknown_entry')fs.unlinkSync(path.join(runDir,'unknown.keep'));else if(variant==='nonempty_unknown_output')fs.unlinkSync(path.join(resultDir,'unknown.keep'));else{fs.unlinkSync(resultFile);fs.writeFileSync(resultFile,original);}
        assert.deepEqual(inventory(editor.input),untouched.input);assert.deepEqual(inventory(path.join(editor.state,'runs',runB)),untouched.other_run);assert.deepEqual(inventory(recovery),untouched.recovery);assert.deepEqual(inventory(external),untouched.external);
        add('xc36_'+variant,{operation_status:rejected.operation_status,no_partial_cleanup:true,protected_inventories_unchanged:true});
      }
      evidence.gaps.push('Native cleanup covers unknown/nonempty output, symlink and external hardlink; deterministic same-ID publication collision requires the test-only ID supplier.');
    });
    await t.test('XC38 native artifact and new Viewer validation deny tampering and a complete foreign run bundle',async()=>{
      const runDir=path.join(editor.state,'runs',runA), resultDir=path.join(runDir,'result'), otherDir=path.join(editor.state,'runs',runB);
      const listing=await editor.api('/api/runs/'+runA+'/artifacts');assert.equal(listing.status,200);const resultArtifact=listing.value.artifacts.find(a=>a.filename==='results.json');assert.ok(resultArtifact,'result artifact exists');
      const originalResult=contents(resultDir), originalResponse=fs.readFileSync(path.join(runDir,'control/worker-response.json'));
      const ledgerBytes=fs.readFileSync(path.join(runDir,'control/record.json')), ledger=JSON.parse(ledgerBytes);
      evidence.valid_runs={a:{run_id:runA,results_sha256:Object.fromEntries([...originalResult].map(([n,b])=>[n,hash(b)])),worker_response_sha256:hash(originalResponse),authoritative_stamp_sha256:hash(JSON.stringify(ledger.stamp))},b:{run_id:runB,results_sha256:Object.fromEntries([...contents(path.join(otherDir,'result'))].map(([n,b])=>[n,hash(b)])),worker_response_sha256:hash(fs.readFileSync(path.join(otherDir,'control/worker-response.json')))}};
      const frozen=inventory(path.join(runDir,'input')), externalBefore=inventory(external), recoveryBefore=inventory(recovery), otherBefore=inventory(otherDir);
      const resultFile=path.join(resultDir,'results.json');
      for(const variant of ['artifact_corruption','manifest_corruption','manifest_absolute_path','manifest_traversal_path','symlink_artifact','hardlink_artifact','foreign_result_and_response']){
        if(variant==='artifact_corruption')fs.appendFileSync(resultFile,'\n');
        if(variant==='manifest_corruption') {const manifest=[...originalResult.keys()].find(n=>n.includes('manifest'));assert.ok(manifest);fs.appendFileSync(path.join(resultDir,manifest),'not-json');}
        if(variant==='manifest_absolute_path'||variant==='manifest_traversal_path'){const name=[...originalResult.keys()].find(n=>n.includes('manifest'));const manifest=JSON.parse(originalResult.get(name));manifest.files.find(f=>f.name==='results.json').name=variant==='manifest_absolute_path'?path.join(external,'sentinel.bin'):'../../external/sentinel.bin';fs.writeFileSync(path.join(resultDir,name),JSON.stringify(manifest));}
        if(variant==='symlink_artifact'||variant==='hardlink_artifact'){fs.unlinkSync(resultFile);if(variant==='symlink_artifact')fs.symlinkSync(path.join(external,'sentinel.bin'),resultFile);else fs.linkSync(path.join(external,'sentinel.bin'),resultFile);}
        if(variant==='foreign_result_and_response'){writeContents(resultDir,contents(path.join(otherDir,'result')));fs.copyFileSync(path.join(otherDir,'control/worker-response.json'),path.join(runDir,'control/worker-response.json'));}
        const artifact=await editor.api(resultArtifact.url);assert.equal(artifact.status,422,variant+' foreign artifact denied');assert.ok(!artifact.text.includes('EXTERNAL-SENTINEL'),'sentinel bytes never served');
        const build=await editor.command('build_viewer',{run_id:runA});assert.equal(build.operation_status,422,variant+' new Viewer denied');
        const now=JSON.parse(fs.readFileSync(path.join(runDir,'control/record.json')));assert.equal(now.run_id,ledger.run_id);assert.deepEqual(now.stamp,ledger.stamp);assert.deepEqual(now.snapshot,ledger.snapshot);
        assert.deepEqual(inventory(path.join(runDir,'input')),frozen,'frozen identity retained');
        writeContents(resultDir,originalResult);fs.writeFileSync(path.join(runDir,'control/worker-response.json'),originalResponse);
        assert.deepEqual(inventory(external),externalBefore);assert.deepEqual(inventory(recovery),recoveryBefore);assert.deepEqual(inventory(otherDir),otherBefore);
        add('xc38_'+variant,{artifact_status:artifact.status,viewer_status:build.operation_status,authoritative_ledger_identity_unchanged:true,frozen_input_unchanged:true,other_run_unchanged:true});
      }
      for(const unsafe of ['..%2F..%2Fexternal%2Fsentinel.bin','%2Fetc%2Fpasswd']){const denied=await editor.api('/api/runs/'+runA+'/artifacts/'+unsafe);assert.ok([400,404,422].includes(denied.status));assert.ok(!denied.text.includes('EXTERNAL-SENTINEL'));add('xc38_unsafe_artifact_path',{status:denied.status});}
      // Recovery must authenticate persisted B response/result against retained A
      // ledger and frozen inputs, rather than adopting B's self-consistent data.
      await editor.stop();writeContents(resultDir,contents(path.join(otherDir,'result')));fs.copyFileSync(path.join(otherDir,'control/worker-response.json'),path.join(runDir,'control/worker-response.json'));await editor.start();
      const recovered=await editor.run(runA);assert.equal(recovered.phase,'integrity_failed');assert.equal(recovered.artifact_state,'invalid');
      const foreign=await editor.api(resultArtifact.url);assert.equal(foreign.status,409);assert.equal(foreign.value.code,'E-EDITOR-RUN-UNPUBLISHED');const build=await editor.command('build_viewer',{run_id:runA});assert.equal(build.operation_status,409);
      add('xc38_foreign_bundle_after_restart',{phase:recovered.phase,artifact_state:recovered.artifact_state,artifact_status:foreign.status,viewer_status:build.operation_status});
      assert.deepEqual(inventory(external),externalBefore);assert.deepEqual(inventory(recovery),recoveryBefore);assert.deepEqual(inventory(editor.input),inputBefore);evidence.input_unchanged=true;
    });
  } catch(error) {throw new Error(redact(error.stack||error.message));}
  finally {
    await editor.stop();
    const link=path.join(editor.input,'models/demo/link');if(fs.existsSync(link)&&fs.lstatSync(link).isSymbolicLink())fs.unlinkSync(link);
    evidence.source_sha256=Object.fromEntries([...contents(editor.input)].map(([n,b])=>[n,hash(b)]));
    const evidenceDir=path.resolve(process.env.DIR_NED_HTTP_EVIDENCE||path.join(root,'evidence'));fs.mkdirSync(evidenceDir,{recursive:true});
    const text=JSON.stringify(evidence,null,2);assert.ok(hidden.every(secret=>!text.includes(secret)),'evidence excludes credentials and capabilities');fs.writeFileSync(path.join(evidenceDir,'native-http-evidence.json'),text+'\n');
    console.log('Native HTTP evidence: '+path.join(evidenceDir,'native-http-evidence.json'));
  }
});
