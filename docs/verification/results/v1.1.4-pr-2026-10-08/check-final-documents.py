from pathlib import Path
import json,re,subprocess,hashlib
BASE=Path('/tmp/dir-v1.1.4-preparation-2026-10-08')
REPO=BASE/'worktree'
plan=json.loads((BASE/'document-version-plan.json').read_text())
rows=plan['documents']+[{'path':'README.md','new_version':'1.1.6'}]
for row in rows:
 path=row['path']
 text=(REPO/path).read_text()
 assert '対象GitHubバージョン：`main @ 2f1e60b`' in text,path
 assert '予定公開版：`v1.1.4`' in text,path
 version=re.search(r'文書バージョン：`(\d+\.\d+\.\d+)`',text).group(1)
 table=text.split('### 更新履歴',1)[1]
 history=re.findall(r'^\| `(\d+\.\d+\.\d+)` \| `?(\d{4}-\d{2}-\d{2})`? \| ([^\n]+) \|$',table,re.M)
 assert history and history[0][0]==version and history[0][1]=='2026-10-08',path
 assert version==row['new_version'],path
 old=subprocess.run(['git','show','origin/main:'+path],cwd=REPO,capture_output=True,text=True)
 if old.returncode==0:
  previous=re.search(r'文書バージョン：`(\d+)\.(\d+)\.(\d+)`',old.stdout)
  nums=tuple(map(int,previous.groups()))
  expected='1.1.0' if nums[:2]!=(1,1) else f'1.1.{nums[2]+1}'
  assert version==expected,(path,version,expected)
  for old_row in re.findall(r'^\| `\d+\.\d+\.\d+` \| [^\n]+$',old.stdout.split('### 更新履歴',1)[1],re.M):
   assert old_row in text,(path,'published history changed')
 else:assert version=='1.1.0',path
summary={'status':'passed','document_versions_checked':len(rows),'all_published_histories_preserved':True}
(BASE/'final-document-verification.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary))
