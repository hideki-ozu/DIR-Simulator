"""Reject comparison changes outside the selected logical priority before CLI use."""
from pathlib import Path
import importlib.util,json,shutil,sys,tempfile
ROOT=Path(__file__).resolve().parents[1]
def main():
 spec=importlib.util.spec_from_file_location('priority_measure',ROOT/'scripts/verify_guide_ethernet_priority.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
 for mutation in ['extra-field','fixed-priority','wrong-priority','ini-time-limit']:
  with tempfile.TemporaryDirectory(prefix='priority-one-variable-') as directory:
   root=Path(directory);source=root/'examples/guide/ethernet-priority';shutil.copytree(ROOT/'examples/guide/ethernet-priority',source)
   if mutation=='ini-time-limit':
    p=source/'priority3.ini';p.write_text(p.read_text().replace('3000000ps','3100000ps'),encoding='utf8')
   else:
    p=source/'priority3.json';x=json.loads(p.read_text())
    if mutation=='extra-field':x['unexpected']=True
    elif mutation=='fixed-priority':x['generators'][2]['priority']=4
    else:x['generators'][1]['priority']=7
    p.write_text(json.dumps(x),encoding='utf8')
   m.ROOT=root;m.SOURCE=source;previous=sys.argv;sys.argv=['checker','--binary',str(root/'nonexistent-cli'),'--output',str(root/'fresh')]
   try:
    try:m.main()
    except AssertionError:pass
    else:raise AssertionError('accepted mutation '+mutation)
   finally:sys.argv=previous
 print('PASS: four extra comparison changes rejected before nonexistent CLI access')
if __name__=='__main__':main()
