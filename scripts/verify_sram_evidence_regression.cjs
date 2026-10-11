// A valid old run must never rescue a missing/incomplete explicitly selected run.
const fs=require('fs'),path=require('path'),assert=require('node:assert/strict'),{spawnSync}=require('child_process');
const {loadEvidence}=require('./sram_evidence.cjs');
const root=path.resolve(process.argv[2]||'.'),fresh=process.argv[3],old=process.argv[4];
assert(fresh&&old,'Usage: node verify_sram_evidence_regression.cjs <root> <fresh> <old>');
loadEvidence(root,old);const selected=loadEvidence(root,fresh);
const missing=path.join(selected.evidence,'missing-run');assert(!fs.existsSync(missing));
for(const checker of ['verify_sram_viewer.cjs','verify_sram_guide_browser.cjs']){
 for(const arg of [undefined,missing]){
  const args=[path.join(root,'scripts',checker),root];if(arg)args.push(arg);
  const run=spawnSync(process.execPath,args,{encoding:'utf8'});
  assert.equal(run.error,undefined);
  assert.equal(run.status,1,checker+' silently accepted missing explicit evidence');
  assert(run.stderr.includes(arg?'missing-run':'CLI-output-directory'),run.stderr);
 }
}
const target=path.join(selected.evidence,'repository/ports1-viewer.html'),backup=target+'.regression-backup';
assert(!fs.existsSync(backup));fs.renameSync(target,backup);
try{
 for(const checker of ['verify_sram_viewer.cjs','verify_sram_guide_browser.cjs']){
  const run=spawnSync(process.execPath,[path.join(root,'scripts',checker),root,fresh],{encoding:'utf8'});
  assert.equal(run.error,undefined);
  assert.equal(run.status,1,checker+' reused old output for incomplete selected run');
  assert(run.stderr.includes('ports1-viewer.html')&&run.stderr.includes('ENOENT'),run.stderr);
 }
}finally{fs.renameSync(backup,target);}
console.log('PASS: both checkers reject omitted, missing and incomplete selected evidence while old valid output remains');
