"""Refuse extra model/INI changes before the formal CLI is ever invoked."""
from pathlib import Path
import importlib.util,json,shutil,sys,tempfile
ROOT=Path(__file__).resolve().parents[1]
def main():
 spec=importlib.util.spec_from_file_location('noc_measure',ROOT/'scripts/verify_guide_noc_input_fifo.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
 for mutation in ['extra-field','source-capacity','wrong-capacity','ini-time-limit']:
  with tempfile.TemporaryDirectory(prefix='noc-one-variable-') as directory:
   root=Path(directory);source=root/'examples/guide/noc-input-fifo';shutil.copytree(ROOT/'examples/guide/noc-input-fifo',source)
   if mutation=='ini-time-limit':
    p=source/'capacity2.ini';p.write_text(p.read_text().replace('90ps','95ps'),encoding='utf8')
   else:
    p=source/'capacity2.json';model=json.loads(p.read_text())
    if mutation=='extra-field':model['unexpected']=True
    elif mutation=='source-capacity':model['source_capacity']=5
    else:model['input_capacity']=1
    p.write_text(json.dumps(model),encoding='utf8')
   module.ROOT=root;module.SOURCE=source;arguments=sys.argv;sys.argv=['checker','--binary',str(root/'nonexistent-cli'),'--output',str(root/'fresh')]
   try:
    try:module.main()
    except AssertionError:pass
    else:raise AssertionError('accepted extra change: '+mutation)
   finally:sys.argv=arguments
 print('PASS: four one-variable mutations rejected before nonexistent CLI access')
if __name__=='__main__':main()
