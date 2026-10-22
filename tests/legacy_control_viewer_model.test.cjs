'use strict';
const test=require('node:test'),assert=require('node:assert/strict');
const fs=require('node:fs'),path=require('node:path'),os=require('node:os');
const {execFileSync}=require('node:child_process');
const C=require('../crates/dir-simulator/src/tool/viewer/assets/controlled-model.js');
const parsers={
 can:require('../crates/dir-simulator/src/tool/viewer/assets/model.js'),
 ethernet:require('../crates/dir-simulator/src/tool/viewer/assets/ethernet-model.js'),
 transaction:require('../crates/dir-simulator/src/tool/viewer/assets/transaction-model.js'),
};
const root=path.resolve(__dirname,'..');
const temp=fs.mkdtempSync(path.join(os.tmpdir(),'dir-legacy-viewer-'));
const fixtures=process.env.DIR_LEGACY_VIEWER_FIXTURES??path.join(temp,'fixtures');
const binary=require('./helpers/simulator_binary.cjs').simulatorBinary(root);
if(!process.env.DIR_LEGACY_VIEWER_FIXTURES){
 execFileSync(process.env.CARGO??'/home/hideki/.cargo/bin/cargo',[
  'test','-p','dir-simulator','--test','legacy_can_ethernet_control','legacy_fixture_exports','--','--exact',
 ],{cwd:root,env:{...process.env,DIR_LEGACY_VIEWER_FIXTURES:fixtures},stdio:'pipe'});
}
test.after(()=>fs.rmSync(temp,{recursive:true,force:true}));

test('all fourteen actual controlled legacy profiles retain family semantics at each boundary',()=>{
 const profiles=fs.readdirSync(fixtures).sort();assert.equal(profiles.length,14);
 for(const profile of profiles)for(const phase of ['before-init','prefix','completed']){
  const input=path.join(fixtures,profile,phase,'results.json');
  const raw=JSON.parse(fs.readFileSync(input)),parsed=C.parseLegacy(raw,parsers);
  assert.equal(parsed.profile,profile);assert.equal(parsed.model.end,BigInt(raw.control.horizon_ps));
  assert.equal(raw.control.state,phase==='completed'?'Completed':'Paused');
  const output=path.join(temp,profile+'-'+phase+'.html');
  execFileSync(binary,['view','--input',input,'--output',output]);
  assert(fs.readFileSync(output,'utf8').includes('legacy-control-banner'));
  const manifest=JSON.parse(fs.readFileSync(path.join(fixtures,profile,phase,'manifest.json')));
  assert.equal(manifest.schema_version,4);assert.equal(manifest.metadata_ref,'results.json#/result/metadata');
  for(const csv of ['events.csv','summary.csv']){
   const lines=fs.readFileSync(path.join(fixtures,profile,phase,csv),'utf8').trim().split(/\r?\n/).map(l=>l.split(','));
   const header=lines.shift(),version=header.indexOf('schema_version');assert(version>=0);
   for(const row of lines)assert.equal(row[version],'4');
  }
 }
});

test('wrapper identity, successful boundary and typed family records reject tampering',()=>{
 const raw=JSON.parse(fs.readFileSync(path.join(fixtures,'can.fd.iso.v1','prefix','results.json')));
 for(const change of [
  r=>r.schema_name='other',r=>r.schema_version=3,
  r=>r.metadata.model_profile='can.cc.ideal.v1',r=>r.metadata.payload_schema_version=1,
  r=>r.control.commit_ordinal='999',r=>r.control.horizon_ps='999',
  r=>r.control.pending_events='999',r=>r.control.resume_supported=false,
  r=>r.control.state='Completed',r=>r.control.replay_sha256='x'.repeat(64),
  r=>r.control.extra=true,r=>r.result.simulation.partial=false,
  r=>r.result.simulation.last_event_time_ps='999',
  r=>r.result.simulation.model_records.find(x=>x.schema_name==='dir.canfd.frame').data.crc^=1,
 ]){
  const value=structuredClone(raw);change(value);assert.throws(()=>C.parseLegacy(value,parsers));
 }
 assert.throws(()=>parsers.can.parseResults(raw.result));
 assert.equal(C.parseLegacy(raw,parsers).model.canfd,true);
});
