"""Prove the shared guide checker refuses commit-decorated target lines."""
from pathlib import Path
import shutil,subprocess,sys,tempfile
from check_noc_input_fifo_document_metadata import ROOT,PATHS
def main():
 with tempfile.TemporaryDirectory(prefix='noc-metadata-') as directory:
  root=Path(directory)
  for name in PATHS:
   dest=root/'docs'/name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(ROOT/'docs'/name,dest)
  checker=ROOT/'scripts/check_noc_input_fifo_document_metadata.py'
  def run():return subprocess.run([sys.executable,'-X','utf8',str(checker),'--root',str(root),'--source-only'],capture_output=True,text=True,encoding='utf-8')
  good=run();assert good.returncode==0,good.stderr
  source=root/'docs'/PATHS[0];original=source.read_text();bad=original.replace('対象GitHubバージョン：`v1.1.4`','対象GitHubバージョン：`v1.1.4`（`b45515644dfc65c205216d564d5bf7ef00280622`）',1);assert bad!=original;source.write_text(bad)
  invalid=run();assert invalid.returncode!=0 and 'target line must contain only the tag' in invalid.stderr,invalid.stderr
  source.write_text(original);restored=run();assert restored.returncode==0,restored.stderr
 print('PASS: real metadata checker accepts tag-only, refuses commit-decorated target, accepts restored source')
if __name__=='__main__':main()
