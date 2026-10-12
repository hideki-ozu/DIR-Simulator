"""Check DDR guide metadata with the real inventory parser and generated rows."""
from pathlib import Path
import argparse
from generate_traceability import GITHUB_VERSION, DOCUMENT_VERSION, document_inventory, Check
ROOT=Path(__file__).resolve().parents[1]
PATHS=('guide/DDRのリフレッシュと読み出し待ち.md','verification/results/ddr-refresh/README.md')
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source-only',action='store_true');a=p.parse_args()
 rows={row[1]:row for row in document_inventory(Check(ROOT))}
 for name in PATHS:
  text=(ROOT/'docs'/name).read_text(encoding='utf-8');match=GITHUB_VERSION.search(text)
  assert match and match.group(1)=='v1.1.4',f'{name}: target line must contain only the tag; put source commit on a separate line'
  version=DOCUMENT_VERSION.search(text).group(1);assert rows[name][2:]==(version,'v1.1.4')
  if not a.source_only:
   for output in ('要件トレーサビリティ一覧.md','要件トレーサビリティ一覧.html'):
    content=(ROOT/'docs'/output).read_text(encoding='utf-8')
    lines=[line for line in content.splitlines() if '>'+rows[name][0]+'</a>' in line]
    assert len(lines)==1 and '<td>'+version+'</td><td>v1.1.4</td>' in lines[0],f'{output}: missing exact DDR document version/tag row'
 print('PASS: real inventory parser and DDR source'+('' if a.source_only else '/both generated rows')+' metadata agree')
if __name__=='__main__':main()
