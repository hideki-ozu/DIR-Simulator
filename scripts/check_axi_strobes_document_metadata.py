"""Check AXI guide metadata with the real inventory parser and generated rows."""
from pathlib import Path
import argparse
from generate_traceability import GITHUB_VERSION, DOCUMENT_VERSION, document_inventory, Check
ROOT=Path(__file__).resolve().parents[1]
PATHS=('guide/AXIの書き込みストローブと部分更新.md','verification/results/axi-strobes/README.md')
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source-only',action='store_true');p.add_argument('--root',type=Path,default=ROOT);a=p.parse_args();root=a.root.resolve()
 rows={row[1]:row for row in document_inventory(Check(root))}
 for name in PATHS:
  text=(root/'docs'/name).read_text(encoding='utf-8');match=GITHUB_VERSION.search(text)
  assert match and match.group(1)=='v1.1.4',f'{name}: target line must contain only the tag; put source commit on a separate line'
  version=DOCUMENT_VERSION.search(text).group(1);assert rows[name][2:]==(version,'v1.1.4')
  if not a.source_only:
   for output in ('要件トレーサビリティ一覧.md','要件トレーサビリティ一覧.html'):
    content=(root/'docs'/output).read_text(encoding='utf-8')
    lines=[line for line in content.splitlines() if '>'+rows[name][0]+'</a>' in line]
    assert len(lines)==1 and '<td>'+version+'</td><td>v1.1.4</td>' in lines[0],f'{output}: missing exact AXI document version/tag row'
 print('PASS: real inventory parser and AXI source'+('' if a.source_only else '/both generated rows')+' metadata agree')
if __name__=='__main__':main()
