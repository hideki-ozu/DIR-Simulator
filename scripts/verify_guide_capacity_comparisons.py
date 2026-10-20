"""Fresh formal v1.1.6 measurements for beginner guides #116--#119.

Runs repository and safely extracted standalone ZIP inputs; refuses old output paths.
The complete portable evidence includes raw results/CSV/manifests, CLI logs and hashes.
"""
from __future__ import annotations
import argparse, copy, hashlib, json, subprocess, zipfile
from pathlib import Path, PurePosixPath
from verify_guide_preemption_fragments import execute, verify_manifest

ROOT=Path(__file__).resolve().parents[1]
COMMIT='397900451015c9bca6a33d24feac0a88b9b7b8fe'
CASES={
 'ethernet-beb-seed':('collision.ini','collision.model.json','seed-1','seed-0'),
 'axi-outstanding':('capacity.ini','capacity.model.json','capacity-1','capacity-2'),
 'ethernet-cbs-gate':('cbs-gated.ini','cbs-gated-model.json','closed','open'),
 'gateway-rx-capacity':('buffered-fanout.ini','buffered-routing.json','rx-1','rx-2'),
}
def load(p):return json.loads(p.read_text())
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def groups(r):
 out={}
 for x in r['simulation']['model_records']:out.setdefault(x['schema_name'],[]).append(x)
 return out

def check_inputs(base,slug):
 ini,model,*names=CASES[slug]; trees=[]
 for name in names:
  d=base/name; rows={p.relative_to(d).as_posix():p.read_bytes() for p in d.rglob('*') if p.is_file()}
  m=load(d/model)
  if slug=='ethernet-beb-seed':assert m['seed']==name[-1];m['seed']='SEED'
  elif slug=='axi-outstanding':assert m['managers'][0]['node']=='Main.m0' and m['managers'][0]['max_outstanding']==int(name[-1]);m['managers'][0]['max_outstanding']=0
  elif slug=='ethernet-cbs-gate':assert m['tsn']['outputs'][0]['port']=='Net.a.tx';assert m['tsn']['outputs'][0]['tas']['entries'][1]['open_priorities']==([] if name=='closed' else [7]);m['tsn']['outputs'][0]['tas']['entries'][1]['open_priorities']=[]
  else:assert m['gateways'][0]['rx_queue_capacity']==int(name[-1]);m['gateways'][0]['rx_queue_capacity']=0
  rows[model]=json.dumps(m,sort_keys=True).encode();trees.append(rows)
 assert trees[0]==trees[1],f'{slug}: more than one experimental variable'
 return {p.relative_to(base).as_posix():sha(p) for p in sorted(base.rglob('*')) if p.is_file()}

def verify_source_inputs(formal_source,base,slug):
 source_rel={'ethernet-beb-seed':'examples/ethernet/media','axi-outstanding':'docs/verification/fixtures/axi','ethernet-cbs-gate':'examples/ethernet/tsn','gateway-rx-capacity':'examples/gateway'}[slug]
 original=formal_source/source_rel
 _,model,*names=CASES[slug]
 for name in names:
  for p in (base/name).rglob('*'):
   if not p.is_file():continue
   rel=p.relative_to(base/name)
   if rel.as_posix()==model:
    expected=load(original/rel)
    if slug=='ethernet-beb-seed':expected['seed']=name[-1]
    elif slug=='axi-outstanding':expected['managers'][0]['max_outstanding']=int(name[-1])
    elif slug=='ethernet-cbs-gate':expected['tsn']['outputs'][0]['tas']['entries'][1]['open_priorities']=[] if name=='closed' else [7]
    else:expected['gateways'][0]['rx_queue_capacity']=int(name[-1])
    assert load(p)==expected,'model differs from formal sample beyond intended field'
   else:assert p.read_bytes()==(original/rel).read_bytes(), 'input differs from formal sample: '+str(rel)
 return {'source_directory':source_rel,'nonvariable_dependencies_byte_equal':True,'model_semantic_equal_except_variable':True,'source_files_sha256':{p.relative_to(original).as_posix():sha(p) for p in (base/names[0]).rglob('*') if p.is_file() for p in [original/p.relative_to(base/names[0])]}}

def run(binary,source,out,slug):
 out.mkdir(parents=True,exist_ok=False);ini,_,*names=CASES[slug];conditions={}
 for name in names:
  d=out/name;d.mkdir();commands=[]
  for args,log in [(['validate','--config',str(source/name/ini)],'validate.log'),(['run','--config',str(source/name/ini),'--output',str(d/'result')],'run.log'),(['view','--input',str(d/'result/results.json'),'--output',str(d/'viewer.html')],'view.log')]:
   _,entry=execute(binary,args,d/log);commands.append(entry)
  manifest=verify_manifest(d/'result');r=load(d/'result/results.json');meta=r['metadata']
  assert meta['git_commit']==COMMIT and meta['git_dirty']=='false' and meta['binary_sha256']==sha(binary)
  conditions[name]={'commands':commands,'manifest':manifest,'results_sha256':sha(d/'result/results.json'),'viewer_sha256':sha(d/'viewer.html'),'metadata':meta,'simulation':r['simulation']}
  if slug=='ethernet-beb-seed':
   _,repeat=execute(binary,['run','--config',str(source/name/ini),'--output',str(d/'repeat')],d/'repeat.log')
   assert load(d/'repeat/results.json')['simulation']==r['simulation'],'same seed logical result differs'
   conditions[name]['same_seed_repeat_logical_equal']=True
   conditions[name]['repeat_results_sha256']=sha(d/'repeat/results.json')
 return conditions

def verify_source(source: Path):
 source=source.resolve()
 commit=subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip()
 dirty=subprocess.check_output(['git','-C',str(source),'status','--porcelain=v1'],text=True).strip()
 assert commit==COMMIT and not dirty, 'formal source must match expected clean commit'
 names=['Cargo.toml','Cargo.lock','rust-toolchain.toml','crates/dir-simulator/Cargo.toml','crates/dir-simulator/build.rs','crates/dir-simulator/build_identity.rs']
 paths=[source/name for name in names]
 for name in ['crates/dir-simulator/src','docs/specs']:paths.extend(p for p in (source/name).rglob('*') if p.is_file())
 ledger=source/'docs/third-party/採用物台帳.md'
 if ledger.is_file():paths.append(ledger)
 digest=hashlib.sha256();files={}
 for p in sorted(paths):
  name=p.relative_to(source).as_posix().encode();raw=p.read_bytes()
  digest.update(len(name).to_bytes(8,'little'));digest.update(name);digest.update(len(raw).to_bytes(8,'little'));digest.update(raw)
  files[name.decode()]={'bytes':len(raw),'sha256':sha(p)}
 return {'source_commit':commit,'git_status_porcelain':dirty,'build_source_sha256':digest.hexdigest(),'files':files}

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--binary',type=Path,required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--only',choices=CASES);p.add_argument('--source-root',type=Path);a=p.parse_args();a.output.mkdir(parents=True,exist_ok=False)
 provenance=verify_source(a.source_root) if a.source_root else None
 if provenance:(a.output/'source-provenance.json').write_text(json.dumps(provenance,ensure_ascii=False,indent=2)+'\n')
 for slug in ([a.only] if a.only else CASES):
  out=a.output/slug;out.mkdir();source=ROOT/'examples/guide'/slug;hashes=check_inputs(source,slug)
  input_origin=verify_source_inputs(a.source_root,source,slug) if a.source_root else None
  repo=run(a.binary.resolve(),source,out/'repository',slug)
  if provenance:assert all(c['metadata']['build_source_sha256']==provenance['build_source_sha256'] for c in repo.values())
  archive=ROOT/'docs/guide/downloads'/f'{slug}-inputs.zip';prefix=f'examples/guide/{slug}/'
  with zipfile.ZipFile(archive) as z:
   expected={prefix+k:v for k,v in hashes.items()};assert set(z.namelist())==set(expected) and len(z.namelist())==len(expected)
   for info in z.infolist():
    q=PurePosixPath(info.filename);assert not q.is_absolute() and '..' not in q.parts
    raw=z.read(info);assert hashlib.sha256(raw).hexdigest()==expected[info.filename]
    target=out/'isolated'/info.filename;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(raw)
  zipped=run(a.binary.resolve(),out/'isolated'/prefix,out/'zip',slug)
  assert all(repo[k]['simulation']==zipped[k]['simulation'] for k in repo),'ZIP logical results differ'
  report={'status':'measured','source_tag':'v1.1.6','source_commit':COMMIT,'binary_sha256':sha(a.binary),'input_sha256':hashes,'zip_sha256':sha(archive),'one_variable_verified':True,'formal_input_origin':input_origin,'zip_logical_equal':True,'conditions':repo,'zip_conditions':zipped}
  (out/'measurements.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
  from check_guide_capacity_measurements import check
  check(slug,out)
  print(f'{slug}: fresh validate/run/view, manifest, one-field diff, ZIP equality PASS')
if __name__=='__main__':main()
