from pathlib import Path
import datetime, hashlib, json, re, shutil, subprocess

BASE = Path('/tmp/dir-pr37-review-2026-10-08')
REPO = Path('/tmp/dir-v1.1.4-preparation-2026-10-08/worktree')
STEM = REPO/'docs/verification/results/pr37-review-2026-10-08'
STEM.mkdir()
def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
rust=json.loads((BASE/'rust-gates/gate.json').read_text())
gates={name:json.loads((BASE/name/'gate.json').read_text()) for name in ['non-rust-gates','python-gates','guide-gates','documentation-gates','example-gates']}
assert rust['status']=='passed' and all(gate['status']=='passed' for gate in gates.values())
for key in ['source_sha256','build_documents_sha256']:
    assert all(sha(REPO/path)==digest for path,digest in rust[key].items())
proof=json.loads((BASE/'historical-proof-pins.json').read_text())
assert all(sha(REPO/path)==digest for path,digest in proof.items())
original=json.loads(Path('/tmp/dir-v1.1.4-preparation-2026-10-08/original-workspace.json').read_text())
root=Path('/home/hideki/DIR-Simulator')
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip()==original['root_head']
for path,row in original['files'].items():
    p=root/path
    assert p.exists()==row['exists'],path
    if row['exists']: assert sha(p)==row['sha256'] and p.stat().st_mode==row['mode'],path
versions=json.loads((BASE/'document-version-plan.json').read_text())
for row in versions['documents']:
    text=(REPO/row['path']).read_text()
    assert f"文書バージョン：`{row['new_version']}`" in text,row['path']
    previous=subprocess.check_output(['git','show',versions['baseline_commit']+':'+row['path']],cwd=REPO,text=True)
    if '### 更新履歴' in previous:
        assert all(line in text for line in re.findall(r'^\| `\d+\.\d+\.\d+` \| [^\n]+$',previous.split('### 更新履歴',1)[1],re.M)),row['path']

selected=[]
for group in ['registry-hash','bridge','sort','rust-gates','python-gates','documentation-gates','non-rust-gates','non-rust-setup-failure']:
    selected.extend((p,p.relative_to(BASE)) for p in (BASE/group).rglob('*') if p.is_file() and 'tmp' not in p.relative_to(BASE/group).parts)
for group in ['guide-gates']:
    selected.extend((p,p.relative_to(BASE)) for p in (BASE/group).iterdir() if p.is_file())
selected.append((BASE/'example-gates/gate.json',Path('example-gates/gate.json')))
for name in ['baseline.json','historical-proof-pins.json','document-version-plan.json','performance-policy-edits.json','edit-performance-policy.py','run-rust-gates.py','run-non-rust-gates.py','verify-shipped-examples.py','final-review.txt','collect-evidence.py']:
    selected.append((BASE/name,Path(name)))
inputs=Path('/tmp/dir-v1.1.4-preparation-2026-10-08/non-rust-gates/configs')
selected.extend((p,Path('integration-inputs')/p.name) for p in inputs.iterdir() if p.is_file())
inventory=[]
for source,relative in sorted(selected,key=lambda pair:str(pair[1])):
    if relative.suffix=='.md': relative=Path(str(relative)+'.txt')
    dest=STEM/relative;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(source,dest)
    assert sha(dest)==sha(source)
    inventory.append({'path':str(relative),'bytes':dest.stat().st_size,'sha256':sha(dest)})

record={'schema_version':1,'document_id':'pr37-review-2026-10-08','pr':37,'related_issue':38,'base_commit':versions['baseline_commit'],'branch':'codex/network-runtime-output-v1.1.4','planned_version':'v1.1.4','status':'local_integration_passed','completed_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source_sha256':rust['source_sha256'],'build_documents_sha256':rust['build_documents_sha256'],'binary':rust['binary'],'rust_gate':rust,'integration_gates':gates,'document_versions':versions,'reproductions':{'registry':{'before':'registry-hash/before-registry.log','regressions_failed':3,'fixed':True},'configuration_hash':{'before':'registry-hash/before-hash.log','regressions_failed':3,'omission':'effective configuration, not raw input','additive_followup_before':'registry-hash/before-additive.log','additive_regressions_failed':2,'fixed':True},'bridge_order':{'before':'bridge/before.log','regressions_failed':1,'fixed':True},'sort_truncation':{'before':'sort/before-whole-row-loss.log','regressions_failed':5,'publication_before':'sort/before-boundary-publication.log','fixed':True}},'review':{'blockers_remaining':0,'report':'final-review.txt','scope':'Registry/hash/sorter/performance policy; parent reviewed Bridge'},'performance_policy':{'mandatory_wall_rss_thresholds':False,'measurement_kind':'reference','million_requests_rerun_this_revision':False,'previous_record':'../can-million-integrated-2026-10-08.md','historical_results_rewritten':False},'preservation':{'historical_proof_files':len(proof),'historical_hashes_match':True,'original_workspace_files':len(original['files']),'original_head_unchanged':True,'original_files_unchanged':True},'publication':{'merge_performed':False,'tag_or_release_created':False,'deployment_performed':False},'support_directory':STEM.name,'support_files':inventory}
Path(str(STEM)+'.json').write_text(json.dumps(record,ensure_ascii=False,indent=2)+'\n')
p=Path(str(STEM)+'.md');text=p.read_text()
text=text.replace('統合検証を実施中。完了後にRust・Node・Python、CLIサンプル、Viewer、文書・ガイドの結果とhashを追記する。','''| 検証 | 結果 |
| --- | --- |
| Rust fmt・Clippy（全targets、警告をerror）・release build | 合格 |
| locked/offline workspace試験 | 566合格、失敗0、ignored0、28 suites |
| Node全体（現行製品capability入力を含む） | 167合格、失敗0、skip0 |
| Python文書試験／ガイド方針 | 39／9合格 |
| 全52出荷INIのvalidate/run/view | 合格、manifestの208ファイルをbyte数・SHA-256照合 |
| Chromium Viewer | 新3profileの完了／部分結果6件、capabilityのCLI生成HTML、CAN/Gateway、transaction5件が合格 |
| strict traceability・生成鮮度・11 PlantUML図 | 231要件・274ノード、構造エラー・未完了0、すべて合格 |
| ガイドCI相当 | 11＋8入力zip、MkDocs strict、9 HTML・409リンク等の検査が合格 |
| 文書版と既存証跡 | 12文書を前回pushから一度改訂。全過去履歴、歴史的証跡2050ファイル、元の作業ツリー2453ファイルを保持 |

修正前の再現ログ、修正後の個別試験、最終統合ログを同名supportディレクトリに収録する。統合用出力親ディレクトリを用意し忘れた初回実行はlocal harness setup errorとして分類し、修正後に全体を実行した。製品の回帰不合格へ数えない。独立レビューの残るblockerは0。既存のすべての受け入れ条件や規格全体への適合を本統合検証だけで合格とはしない。

バイナリは修正を加えたdirty worktreeから構築し、基準commitとsource/specのhashで識別する。統合検証後にコードと構築入力のhash一致を確認した。バイナリと52サンプルの全成果物はローカル検証ディレクトリに保持し、本記録には照合値を保存する。''')
p.write_text(text)
print(json.dumps({'status':record['status'],'support_files':len(inventory),'support_bytes':sum(row['bytes'] for row in inventory),'rust_tests':rust['tests']['passed'],'node_tests':gates['non-rust-gates']['node_counts']['pass'],'examples':gates['example-gates']['total_cases'],'current_document_versions':len(versions['documents'])}))
