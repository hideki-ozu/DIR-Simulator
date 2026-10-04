'use strict';
const test=require('node:test'),assert=require('node:assert/strict'),path=require('node:path');
const {simulatorBinary}=require('./helpers/simulator_binary.cjs');
const repository=path.resolve(__dirname,'..');
test('viewer model launcher builds through PATH Cargo with the platform executable name',()=>{
 for(const platform of ['linux','win32']){
  const calls=[];
  const binary=simulatorBinary(repository,{env:{},platform,execute:(...args)=>calls.push(args)});
  assert.deepEqual(calls,[['cargo',['build','--locked','-p','dir-simulator'],{cwd:repository,stdio:'pipe'}]]);
  assert.equal(binary,path.join(repository,'target','debug',platform==='win32'?'dir-simulator.exe':'dir-simulator'));
 }
});
test('viewer model launcher honors CARGO and an existing DIR_SIMULATOR_BIN without rebuilding',()=>{
 const calls=[],cargo=path.join(repository,'toolchain with spaces','cargo');
 simulatorBinary(repository,{env:{CARGO:cargo},execute:(...args)=>calls.push(args)});
 assert.equal(calls[0][0],cargo);
 const binary=simulatorBinary(repository,{env:{CARGO:'missing-cargo',DIR_SIMULATOR_BIN:'prebuilt/simulator'},execute:()=>assert.fail('prebuilt executable must bypass Cargo')});
 assert.equal(binary,path.resolve(repository,'prebuilt/simulator'));
});
