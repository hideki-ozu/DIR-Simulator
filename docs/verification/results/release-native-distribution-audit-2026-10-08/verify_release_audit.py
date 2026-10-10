import base64, hashlib, io, json, platform, tomllib, zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
data = json.loads((ROOT / 'audit-inputs.json').read_text(encoding='utf-8'))
files = data['files']
def raw(path):
    f = files[path]
    return base64.b64decode(f['content']) if f['encoding'] == 'base64' else f['content'].encode('utf-8')
def sha(b):
    return hashlib.sha256(b).hexdigest()
blobs = []
for path, f in files.items():
    b = raw(path)
    actual = hashlib.sha1(b'blob ' + str(len(b)).encode() + b'\0' + b).hexdigest()
    blobs.append({'path': path, 'bytes': len(b), 'sha256': sha(b), 'git_blob_sha1': actual, 'expected_blob_sha1': f['sha'], 'match': actual == f['sha']})
assert all(b['match'] for b in blobs)
archives = []
for profile in ('can', 'canfd'):
    path = f'docs/guide/downloads/{profile}-guide-inputs.zip'
    expected = {p: raw(p) for p in files if p.startswith(f'examples/guide/{profile}/')}
    z = zipfile.ZipFile(io.BytesIO(raw(path)))
    members = [{'path': n, 'bytes': len(z.read(n)), 'sha256': sha(z.read(n)), 'source_equal': n in expected and z.read(n) == expected[n]} for n in z.namelist()]
    match = len(z.namelist()) == len(set(z.namelist())) and set(z.namelist()) == set(expected) and all(m['source_equal'] for m in members)
    assert match
    archives.append({'path': path, 'sha256': sha(raw(path)), 'exact_members_and_bytes_match': match, 'members': members, 'scope': 'input-only exercise zip; not a full product release archive'})
licenses = []
for name in ('LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE'):
    bundled = f'docs/guide/assets/licenses/{name}.txt'
    equal = raw(name) == raw(bundled)
    assert equal
    licenses.append({'source': name, 'bundled': bundled, 'sha256': sha(raw(name)), 'bytes_equal': equal})
upstream = []
for name, result in data['upstream'].items():
    filename = 'fontawesome-license.txt' if name == 'fontawesome' else name + '-MIT.txt'
    local = f'docs/guide/assets/licenses/{filename}'
    equal = raw(local) == result['content'].encode('utf-8')
    assert equal
    upstream.append({'name': name, 'url': result['url'], 'bundled': local, 'sha256': sha(raw(local)), 'bytes_equal': equal, 'scope': 'license text only; bundled asset version/hash and obligations not fully reviewed'})
packages = tomllib.loads(raw('Cargo.lock').decode())['package']
external = [p for p in packages if 'source' in p]
deps = tomllib.loads(raw('crates/dir-simulator/Cargo.toml').decode())['dependencies']
ledger = 'docs/third-party/採用物台帳.md'
tree_paths = {x['path'] for x in data['tree']}
ledger_scan = data['ledger_tree_scan']
assert ledger_scan['truncated'] is False
complete_tree_paths = set(ledger_scan['paths'])
assert '\ufffd' not in ledger
assert ledger == 'docs/third-party/' + ''.join(map(chr, (0x63a1, 0x7528, 0x7269, 0x53f0, 0x5e33))) + '.md'
assert not any('\ufffd' in p for p in complete_tree_paths)
assert (ledger in tree_paths) == (ledger in complete_tree_paths)
prefix = 'docs/verification/results/v1.1.4-pr-2026-10-08/non-rust-gates/results/bridge-complete/'
manifest = json.loads(raw(prefix + 'manifest.json'))
product = json.loads(raw(prefix + 'results.json'))
manifest_checks = []
for f in manifest['files']:
    b = raw(prefix + f['name'])
    ok = len(b) == int(f['bytes']) and sha(b) == f['sha256']
    assert ok
    manifest_checks.append({'path': prefix + f['name'], 'bytes': len(b), 'sha256': sha(b), 'match': ok})
assert manifest['metadata_ref'] == 'results.json#/metadata'
metadata = product['metadata']
report = {
    'schema_version': 1, 'recorded_date_utc': '2026-10-08', 'target_commit': data['commit'],
    'overall_status': 'partial_verification_release_conditions_remain',
    'environment': {'os': platform.platform(), 'python': platform.python_version(), 'native_simulator_executed': False},
    'rules_read': ['docs/ドキュメント作成・運用規約.md', 'docs/トレーサビリティ管理規約.md', 'docs/品質・配布方針.md', 'docs/要件定義書.md#dir-ac-0008', 'docs/verification/cases/利用フロー・品質検証仕様書.md#dir-test-0083', 'docs/verification/cases/利用フロー・品質検証仕様書.md#dir-test-0087', 'docs/verification/results/acceptance-2026-10-08.md', 'docs/verification/results/v1.1.4-pr-2026-10-08.md'],
    'instructions_inventory': {'tracked_AGENTS': sorted(p for p in tree_paths if p.endswith('AGENTS.md')), 'tracked_agent_skills': sorted(p for p in tree_paths if p.startswith('.agents/')), 'local_current_workspace_AGENTS_found': False, 'untracked_original_repo_instructions': 'not available; no original denied filesystem access bypass'},
    'blob_checks': blobs, 'input_archives': archives, 'project_license_copies': licenses, 'fixed_tag_upstream_license_checks': upstream,
    'dependency_inventory': {'lock_sha256': sha(raw('Cargo.lock')), 'external_locked_package_count': len(external), 'direct_runtime_dependencies': sorted(deps), 'direct_build_dependencies': ['sha2'], 'packages': external, 'ledger_path': ledger, 'ledger_present': ledger in complete_tree_paths, 'unmapped_external_package_count': len(external)},
    'ledger_tree_verification': {'ref': ledger_scan['ref'], 'response_sha': ledger_scan['response_sha'], 'truncated': ledger_scan['truncated'], 'tree_path_count': len(complete_tree_paths), 'checked_path': ledger, 'path_exists': ledger in complete_tree_paths, 'third_party_entries': sorted(p for p in complete_tree_paths if p.startswith('docs/third-party/')), 'adoption_named_entries': sorted(p for p in complete_tree_paths if '採用物' in p)},
    'encoding_correction': {'previous_evidence_head': '3309dbdb08e76eb3690d108790a0ad4fa7040c1b', 'cause': 'PowerShell default decoding corrupted UTF-8 source and report during remote submission', 'input_snapshot_was_valid_utf8': True, 'remote_readback_required': True},
    'historical_product_manifest': {'path': prefix+'manifest.json', 'files': manifest_checks, 'metadata_ref': manifest['metadata_ref'], 'metadata_ref_resolves': True, 'adoption_ledger_version': metadata.get('adoption_ledger_version'), 'adoption_ledger_sha256': metadata.get('adoption_ledger_sha256'), 'ledger_reference_resolves': False, 'scope': 'independent byte/hash verification of preserved results; no current simulator execution'},
    'main_exact_head_ci': data['ci'],
    'native_comparison': {'status': 'blocked_not_performed', 'baseline': 'preserved records use Ubuntu 24.04.4 WSL2; rustc 1.85.0', 'execution_count': 0, 'normalization_contract': {'json': 'typed simulation comparison; preserve array order', 'csv': 'remove run_id only; preserve all row/value order', 'environment': 'separate run_id, wall timestamps/performance and absolute paths', 'integers_order_counts': 'exact equality', 'derived_ratios': 'absolute <=1e-12 OR relative <=1e-9'}, 'performance_wall_rss': 'reference only; not mandatory pass/fail'},
    'blockers': ['wsl --status exit 1: Wsl/EnumerateDistros/Service/E_ACCESSDENIED', 'existing checkout git status rejected dubious ownership: owner OMEN35L/rxg03; caller OMEN35L/CodexSandboxOffline; no safe.directory or permission changes', 'cargo/rustc not available through Get-Command; native Ubuntu execution surface unavailable', 'adoption ledger absent despite 42 external locked packages', 'historical result ledger hash not-present and version is Git commit, not an existing ledger version', 'full fixed product distribution archive not supplied; input zips cannot substitute', 'guide third-party texts lunr-languages use mutable master URL; TinySegmenter URL has no version; acquisition hashes/reviewer decisions remain absent', 'vendored/generated/fixture materials provenance and generated MkDocs transitive distribution inventory remain incomplete'],
    'acceptance': {'DIR-AC-0008': 'not_satisfied', 'DIR-TEST-0087': 'partial_not_pass', 'DIR-TEST-0083_native_WSL2_comparison': 'not_run'},
    'prohibited_actions_performed': []
}
assert all('\ufffd' not in p and p.split('#')[0] in complete_tree_paths for p in report['rules_read'])
assert '\ufffd' not in json.dumps(report, ensure_ascii=False)
(ROOT / 'release-audit-2026-10-08.json').write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
print(json.dumps({'blob_checks': len(blobs), 'zip_members': [len(a['members']) for a in archives], 'external_packages': len(external), 'project_license_copies': len(licenses), 'upstream_license_texts': len(upstream), 'historical_manifest_files': len(manifest_checks), 'overall_status': report['overall_status']}, ensure_ascii=False))
