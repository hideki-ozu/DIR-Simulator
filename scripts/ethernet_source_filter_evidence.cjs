// Bind browser checks to the explicit fresh CLI output, current inputs and formal provenance.
const fs=require('fs'),path=require('path'),crypto=require('crypto'),assert=require('node:assert/strict');
const sha=raw=>crypto.createHash('sha256').update(raw).digest('hex');
function loadEvidence(root,argument){
 assert(argument,'Usage: node <checker.cjs> <repository-root> <CLI-output-directory>');
 const evidence=path.resolve(root,argument),inputHashes={};
 const measurement=JSON.parse(fs.readFileSync(path.join(evidence,'measurements.json'),'utf8'));
 assert.equal(measurement.status,'passed');assert.equal(measurement.source_tag,'v1.1.4');
 assert.deepEqual(measurement,JSON.parse(fs.readFileSync(path.join(root,'docs/verification/results/ethernet-source-filter/measurements.json'),'utf8')));
 for(const [name,hash] of Object.entries(measurement.public_input_sha256)) assert.equal(sha(fs.readFileSync(path.join(root,name))),hash);
 for(const name of ['include','exclude']){
  const directory=path.join(evidence,'repository',name);
  const manifest=JSON.parse(fs.readFileSync(path.join(directory,'manifest.json'),'utf8'));
  assert.equal(manifest.status,'complete');assert.equal(manifest.partial,false);
  for(const entry of manifest.files){const raw=fs.readFileSync(path.join(directory,entry.name));assert.equal(String(raw.length),entry.bytes);assert.equal(sha(raw),entry.sha256);}
  const doc=JSON.parse(fs.readFileSync(path.join(directory,'results.json'),'utf8'));
  assert.deepEqual(doc.simulation,measurement.conditions[name]);
  assert.equal(doc.metadata.git_commit,measurement.source_commit);assert.equal(String(doc.metadata.git_dirty),'false');
  assert.equal(doc.metadata.binary_sha256,measurement.provenance[name].binary_sha256);
  for(const filename of [`repository/${name}-viewer.html`,`repository/${name}/results.json`,`repository/${name}/manifest.json`]) inputHashes[filename]=sha(fs.readFileSync(path.join(evidence,filename)));
 }
 return {evidence,inputHashes};
}
module.exports={loadEvidence};
