"""Reject extra VLAN comparison changes before any CLI access."""
from pathlib import Path
import importlib.util,json,shutil,sys,tempfile
ROOT=Path(__file__).resolve().parents[1]
def main():
 spec=importlib.util.spec_from_file_location('vlan_measure',ROOT/'scripts/verify_guide_ethernet_vlan.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
 for mutation in ['extra-field','other-port','wrong-admit','ini-time-limit']:
  with tempfile.TemporaryDirectory(prefix='vlan-one-variable-') as directory:
   root=Path(directory);source=root/'examples/guide/ethernet-vlan';shutil.copytree(ROOT/'examples/guide/ethernet-vlan',source)
   if mutation=='ini-time-limit':
    p=source/'tagged.ini';p.write_text(p.read_text().replace('2000000ps','2100000ps'),encoding='utf8')
   else:
    p=source/'tagged.json';x=json.loads(p.read_text())
    if mutation=='extra-field':x['unexpected']=True
    elif mutation=='other-port':x['ports'][0]['admit']='tagged_only'
    else:x['ports'][3]['admit']='all'
    p.write_text(json.dumps(x),encoding='utf8')
   m.ROOT=root;m.SOURCE=source;previous=sys.argv;sys.argv=['checker','--binary',str(root/'nonexistent-cli'),'--output',str(root/'fresh')]
   try:
    try:m.main()
    except AssertionError:pass
    else:raise AssertionError('accepted mutation '+mutation)
   finally:sys.argv=previous
 print('PASS: four extra comparison changes rejected before nonexistent CLI access')
if __name__=='__main__':main()
