"""Check shared-memory guide metadata with the real inventory parser and generated rows."""
from pathlib import Path
import argparse
from generate_traceability import GITHUB_VERSION, DOCUMENT_VERSION, document_inventory, Check
ROOT=Path(__file__).resolve().parents[1]
PATHS=('guide/共有メモリの公開と取得権限.md','verification/results/shared-roles/README.md')
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source-only',action='store_true');p.add_argument('--root',type=Path,default=ROOT);a=p.parse_args();root=a.root.resolve()
 rows={row[1]:row for row in document_inventory(Check(root))}
 root_target=GITHUB_VERSION.search((root/'README.md').read_text(encoding='utf-8'))
 assert root_target, 'README.md: missing canonical target tag'
 active_tag=root_target.group(1)
 for name in PATHS:
  expected_target=active_tag if name.startswith('guide/') else 'v1.1.4'
  text=(root/'docs'/name).read_text(encoding='utf-8');match=GITHUB_VERSION.search(text)
  assert match and match.group(1)==expected_target,f'{name}: target line must contain only the tag; put source commit on a separate line'
  version=DOCUMENT_VERSION.search(text).group(1);assert rows[name][2:]==(version,expected_target)
  if not a.source_only:
   for output in ('要件トレーサビリティ一覧.md','要件トレーサビリティ一覧.html'):
    content=(root/'docs'/output).read_text(encoding='utf-8')
    lines=[line for line in content.splitlines() if '>'+rows[name][0]+'</a>' in line]
    assert len(lines)==1 and '<td>'+version+'</td><td>'+expected_target+'</td>' in lines[0],f'{output}: missing exact shared-memory document version/tag row'
 print('PASS: real inventory parser and shared-memory source'+('' if a.source_only else '/both generated rows')+' metadata agree')
if __name__=='__main__':main()
