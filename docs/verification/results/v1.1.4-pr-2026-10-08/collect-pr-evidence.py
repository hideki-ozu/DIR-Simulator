from pathlib import Path
import datetime,hashlib,json,shutil,subprocess
BASE=Path('/tmp/dir-v1.1.4-preparation-2026-10-08')
REPO=BASE/'worktree'
ORIGINAL=Path('/home/hideki/DIR-Simulator')
STEM='v1.1.4-pr-2026-10-08'
RESULTS=REPO/'docs/verification/results'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def read(p):return json.loads(p.read_text())
def main():
 rust=read(BASE/'rust-gates/gate.json')
 nonrust=read(BASE/'non-rust-gates/validation-summary.json')
 docs=read(BASE/'documentation-gates/gate.json')
 guide=read(BASE/'guide-gates/gate.json')
 examples=read(BASE/'example-gates/gate.json')
 versions=read(BASE/'document-version-plan.json')
 versionproof=read(BASE/'final-document-verification.json')
 snapshot=read(BASE/'original-workspace.json')
 assert all(x['status']=='passed' for x in (rust,docs,guide,examples,versionproof))
 assert nonrust['status']=='complete'
 assert nonrust['final_counts']['node_tests']=={'tests':167,'passed':167,'failed':0,'skipped':0}
 assert rust['tests']=={'passed':543,'failed':0,'ignored':0,'suites':28}
 assert examples['total_cases']==52 and examples['total_manifest_files']==208
 assert versionproof['document_versions_checked']==49
 for rel,h in rust['source_sha256'].items():assert sha(REPO/rel)==h,rel
 for rel,h in rust['build_documents_sha256'].items():assert sha(REPO/rel)==h,rel
 assert sha(Path(rust['binary']['path']))==rust['binary']['sha256']
 assert subprocess.check_output(['git','rev-parse','origin/main'],cwd=REPO,text=True).strip()==snapshot['integration_base']
 assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=ORIGINAL,text=True).strip()==snapshot['root_head']
 for rel,r in snapshot['files'].items():
  p=ORIGINAL/rel
  assert p.exists()==r['exists'],rel
  if r['sha256']:assert sha(p)==r['sha256'],rel
 proofpaths=[rel for rel in snapshot['files'] if rel.startswith('docs/verification/results/')]
 for rel in proofpaths:assert sha(REPO/rel)==snapshot['files'][rel]['sha256'],rel
 csv=read(BASE/'preserved-evidence-csv.json')
 for entry in csv:assert sha(REPO/entry['path'])==entry['sha256']
 original_million=read(REPO/'docs/verification/results/can-million-integrated-2026-10-08.json')
 source_difference=[rel for rel,h in original_million['source_sha256'].items() if rust['source_sha256'].get(rel)!=h]
 assert sorted(source_difference)==['crates/dir-simulator/src/tool/viewer/assets/transaction-app.js','crates/dir-simulator/src/tool/viewer/assets/transaction-model.js']
 report_path=RESULTS/(STEM+'.json')
 md_path=RESULTS/(STEM+'.md')
 support=RESULTS/STEM
 assert not report_path.exists() and not md_path.exists() and not support.exists()
 support.mkdir()
 inventory=[]
 def add(source,relative,kind):
  assert source.is_file() and not source.is_symlink(),source
  dest=support/relative
  assert not dest.exists()
  dest.parent.mkdir(parents=True,exist_ok=True)
  shutil.copyfile(source,dest)
  assert sha(source)==sha(dest)
  inventory.append({'kind':kind,'original_local_path':str(source),'support_relative_path':relative,'bytes':dest.stat().st_size,'sha256':sha(dest)})
 for folder in ('rust-gates','non-rust-gates','documentation-gates','guide-gates'):
  for p in sorted((BASE/folder).rglob('*')):
   if not p.is_file() or '__pycache__' in p.parts:continue
   rel=str(p.relative_to(BASE))
   if p.suffix=='.md':rel+='.txt'
   add(p,rel,folder)
 for rel in ('example-gates/gate.json','document-version-plan.json','final-document-verification.json','original-workspace.json','preserved-evidence-csv.json','run-rust-gates.py','verify-shipped-examples.py','check-final-documents.py','collect-pr-evidence.py'):
  add(BASE/rel,rel,'preparation_provenance')
 add(REPO/'.gitattributes','archival-whitespace-attributes.txt','byte_preservation_policy')
 report={'schema_version':1,'document_id':'v1-1-4-pr-2026-10-08','planned_github_version':'v1.1.4','previous_published_version':'v1.1.3','cargo_package_version':'0.1.0','status':'local_validation_passed','prepared_at_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'branch':subprocess.check_output(['git','branch','--show-current'],cwd=REPO,text=True).strip(),'base_commit':snapshot['integration_base'],'original_workspace_commit':snapshot['root_head'],'source_sha256':rust['source_sha256'],'build_documents_sha256':rust['build_documents_sha256'],'binary':rust['binary'],'rust_gate':rust,'non_rust_validation':nonrust,'documentation_gate':docs,'guide_gate':guide,'shipped_examples':examples,'document_versions':versions,'document_version_verification':versionproof,'preservation':{'original_workspace_files_verified':len(snapshot['files']),'copied_historical_proof_files_verified':len(proofpaths),'explicitly_included_csv_evidence_files':csv,'old_measurement_records_rewritten':False},'integration_review':{'scope':'Focused source/interface/main-integration and final release prose review; not exhaustive standards conformance.','blockers_remaining':0,'retained_main_features':['Issue #20 transaction rejection timeline fix','CAN/CAN FD guides and samples','Guide publication/check workflows'],'historical_record_language':'Historical verification applies to its recorded fixed source; current gates are captured separately.'},'million_performance':{'record':'can-million-integrated-2026-10-08.json','record_sha256':sha(REPO/'docs/verification/results/can-million-integrated-2026-10-08.json'),'scope':'Previously recorded fixed-source WSL2 single completion observations per condition; not measured again during PR preparation.','current_source_differences_from_measured_source':source_difference,'current_build_documents_differ_due_to_release_document_updates':True,'observed_time_target_met':False,'observed_rss_target_met':True,'formal_protocol_complete':False,'native_baseline_verdict':'unverified'},'support_directory':STEM,'inventory_base_directory':STEM,'support_files':sorted(inventory,key=lambda x:x['support_relative_path'])}
 report_path.write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
 md='''# v1.1.4向けPR準備・統合検証記録（2026-10-08）

文書ID：`v1-1-4-pr-2026-10-08`

公開済みv1.1.3からPATCHを一つ上げ、予定公開版をv1.1.4とする。最新main `2f1e60b`からブランチ`codex/network-runtime-output-v1.1.4`を作成し、公開Registry/runtime・構造化診断・入力と構築の来歴・準備失敗出力、CAN/Gateway逐次処理と出力最適化①〜⑤、CAN↔Ethernet変換・動的制御・TSN、そのViewer・サンプル・検証記録を統合した。GitHubタグとCargoパッケージ版は別管理とし、Cargo 0.1.0を維持する。本記録取得時にv1.1.4のタグ・Releaseは作成していない。

## 現行ソースでの検証

| 検証 | 結果 |
| --- | --- |
| Rust fmt・Clippy（locked/offline、全targets、警告をerror） | 合格 |
| Rust workspace（locked/offline） | 543合格、失敗0、ignored0、28 suites |
| release build | 合格 |
| Node全体（実製品のcapability結果も入力） | 167合格、失敗0、skip0 |
| Python文書試験／ガイドscope policy試験 | 39／9合格 |
| 出荷サンプルのCLI validate/run/view | 全52 INI合格、manifestの208ファイルをbyte数・SHA-256照合 |
| Chromium Viewer | 新規3 profileの完了／部分結果6件、CAN/Gateway、transaction5件、実製品capabilityのCLI生成HTMLが合格 |
| strict traceability・生成鮮度 | 231要件・60機能・274ノード、構造エラー・未完了0 |
| PlantUML鮮度 | 11図合格 |
| ガイドCIと同じ入力zip検査・MkDocs strict・リンク検査 | 11＋8入力、9 HTML、409リンク／asset／anchor、すべて合格 |
| 文書版・更新履歴 | 現行49文書を公開済みmainから採番、過去公開履歴を保持 |

transaction ViewerではIssue #20の修正をmainから維持し、実シミュレータで0 psに拒否された要求の生成0・開始なし・完了0を確認してブラウザへ入力した。新規3 profileはmax-eventsによる実行失敗のpartial結果もViewerへ入力した。動的／TSNの宣言capabilityと実効membershipを分ける製品結果を用い、最初に環境入力不足でskipしたNode試験を有効にして全体を再実行した。初回ログも保持する。

## 版と証跡の扱い

READMEの文書版はmainの1.1.5から1.1.6へ進めた。編集した各文書の版はpush単位で一度だけ更新し、新規文書は1.1.0、1.0系列から対象MAJOR.MINORを変更した文書も1.1.0とした。仕様の対象は公開済み`main @ 2f1e60b`、予定公開版はv1.1.4と記す。生成一覧はステージ済み正本から既存pre-commit hookで再生成した。

元の作業ツリー2453ファイルをhash照合して保持し、過去の検証証跡1812ファイルを変更せずコピーした。manifestへ収録された16 CSVファイルは通常のCSV除外規則にかかわらず明示的に収録した。生ログ・Git差分のcontext空白・perfレポートはSHA-256の一致を保つため取得時のbyte列を維持し、`.gitattributes`では該当する歴史的tool出力だけを空白書式検査の対象外とする。製品ソースと編集用文書の検査は維持する。

[100万要求の完走記録](can-million-integrated-2026-10-08.md)は取得時の固定ソース・バイナリ・入力のまま保存する。WSL2の通常／高負荷／過負荷各1回で754.75／740.48／592.35秒、47.41／49.06／47.88 MiB。120秒の時間目標は未達、完走観測のRSSは2 GiB以内で、正式3反復・native基準機の合否は未検証である。今回の版準備で100万要求を再測定したとは扱わない。測定時からの製品ソース差はmainのtransaction Viewer2ファイルで、構築入力の仕様書は版・現行状態の整理により変わっている。規格全体適合・全受入条件の合格も主張しない。

[機械可読記録](v1.1.4-pr-2026-10-08.json)に現行source/specificationのhash、測定済みrelease binaryのhash、各gateとコマンド、全52サンプルの照合値を保存した。同名supportディレクトリへ生ログ、入力、ブラウザで検証した成果物、版採番計画を収録し、inventoryのbyte数・SHA-256で照合した。サンプル52件の全結果ファイルとバイナリはローカルのPR準備ディレクトリへ保持し、本記録には照合値を保存する。
'''
 md_path.write_text(md)
 expected={r['support_relative_path']:r for r in report['support_files']}
 actual={str(p.relative_to(support)):p for p in support.rglob('*') if p.is_file()}
 assert set(actual)==set(expected)
 for rel,p in actual.items():assert p.stat().st_size==expected[rel]['bytes'] and sha(p)==expected[rel]['sha256']
 print(json.dumps({'status':'passed','report':str(report_path),'support_files':len(inventory),'source_pins':len(rust['source_sha256']),'build_doc_pins':len(rust['build_documents_sha256'])}))
if __name__=='__main__':main()
