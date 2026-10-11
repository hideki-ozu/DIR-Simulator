// Require an explicit CLI output directory; never fall back to another run.
const fs=require('fs'),path=require('path'),crypto=require('crypto'),assert=require('node:assert/strict');
function loadEvidence(root,argument){
 assert(argument,'Usage: node <checker.cjs> <repository-root> <CLI-output-directory>');
 const evidence=path.resolve(root,argument);
 assert(fs.statSync(evidence).isDirectory(),'CLI output must be a directory');
 const inputHashes={};
 for(const ports of [1,2,3]){
  for(const name of [`repository/ports${ports}-viewer.html`,`repository/ports${ports}/results.json`,`repository/ports${ports}/manifest.json`]){
   const file=path.join(evidence,name);
   assert(fs.statSync(file).isFile(),`Missing required evidence: ${name}`);
   inputHashes[name]=crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
  }
  const directory=path.join(evidence,`repository/ports${ports}`);
  const manifest=JSON.parse(fs.readFileSync(path.join(directory,'manifest.json'),'utf8'));
  assert.equal(manifest.status,'complete');assert.equal(manifest.partial,false);
  for(const entry of manifest.files){
   const raw=fs.readFileSync(path.join(directory,entry.name));
   assert.equal(String(raw.length),entry.bytes);
   assert.equal(crypto.createHash('sha256').update(raw).digest('hex'),entry.sha256);
  }
 }
 return {evidence,inputHashes};
}
module.exports={loadEvidence};
