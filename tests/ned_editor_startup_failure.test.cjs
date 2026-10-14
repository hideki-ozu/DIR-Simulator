'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawnSync} = require('node:child_process');

test('forced second startup failure reports child skips and parent failure', {skip:process.env.DIR_NED_HTTP_BINARY?false:'Requires built native executable',timeout:180000}, t=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'ned-startup-report-'));
  t.after(()=>fs.rmSync(root,{recursive:true,force:true}));
  const preload=path.join(root,'force-restart.cjs');
  const helper=require.resolve('./ned_editor_startup.cjs');
  fs.writeFileSync(preload,`const helper=require(${JSON.stringify(helper)});const original=helper.waitForStartup;let attempts=0;helper.waitForStartup=(child,options)=>original(child,++attempts===2?{...options,timeoutMs:1}:options);`);
  const env={...process.env,DIR_NED_HTTP_EVIDENCE:path.join(root,'evidence')};
  delete env.NODE_TEST_CONTEXT;
  const run=spawnSync(process.execPath,['--require',preload,'--test',require.resolve('./ned_editor_acceptance_http.test.cjs')],{
    env,encoding:'utf8',timeout:170000,maxBuffer:2*1024*1024
  });
  assert.equal(run.error,undefined);assert.equal(run.status,1,'intentional restart failure must fail suite');
  const output=run.stdout;
  assert.match(output,/not ok 3 - XC39/);
  assert.match(output,/ok 4 - XC36[^\n]*# SKIP/);
  assert.match(output,/ok 5 - XC38[^\n]*# SKIP/);
  assert.match(output,/not ok 1 - native HTTP acceptance/);
  assert.doesNotMatch(output,/not ok 1 - native HTTP acceptance[^\n]*# SKIP/);
  assert.match(output,/# pass 2\b/);assert.match(output,/# fail 2\b/);assert.match(output,/# skipped 2\b/);
  const evidence=JSON.parse(fs.readFileSync(path.join(root,'evidence/native-http-evidence.json'),'utf8'));
  const failed=evidence.startups[1];assert.equal(failed.state,'startup_timeout');
  assert.ok(failed.exit_code!==null||failed.signal!==null,'failed child reaped');
  console.log('FORCED_STARTUP_REPORT '+JSON.stringify({inner_exit:run.status,pass:2,fail:2,skipped:2,startup_state:failed.state,owned_child_reaped:true,controlled_timeout_ms:1,product_default_timeout_ms:20000}));
});
