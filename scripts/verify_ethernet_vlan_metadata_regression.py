"""Prove the shared guide checker refuses commit-decorated target lines."""
from pathlib import Path
import shutil,subprocess,sys,tempfile
from check_ethernet_vlan_document_metadata import ROOT,PATHS
from generate_traceability import GITHUB_VERSION
def main():
 current=GITHUB_VERSION.search((ROOT/'README.md').read_text(encoding='utf-8'))
 assert current, 'README.md: missing canonical target tag'
 current_tag=current.group(1)
 with tempfile.TemporaryDirectory(prefix='ethernet-vlan-metadata-') as directory:
  root=Path(directory)
  shutil.copyfile(ROOT/'README.md',root/'README.md')
  for name in PATHS:
   dest=root/'docs'/name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(ROOT/'docs'/name,dest)
  checker=ROOT/'scripts/check_ethernet_vlan_document_metadata.py'
  def run():return subprocess.run([sys.executable,'-X','utf8',str(checker),'--root',str(root),'--source-only'],capture_output=True,text=True,encoding='utf-8')
  good=run();assert good.returncode==0,good.stderr
  source=root/'docs'/PATHS[0];original=source.read_text();tag_line=f'対象GitHubバージョン：`{current_tag}`';bad=original.replace(tag_line,tag_line+'（`b45515644dfc65c205216d564d5bf7ef00280622`）',1);assert bad!=original;source.write_text(bad)
  invalid=run();assert invalid.returncode!=0 and 'target line must contain only the tag' in invalid.stderr,invalid.stderr
  source.write_text(original);restored=run();assert restored.returncode==0,restored.stderr
 print('PASS: real metadata checker accepts tag-only, refuses commit-decorated target, accepts restored source')
if __name__=='__main__':main()
