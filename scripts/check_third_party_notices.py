"""Check supplied originals, lock coverage and the notices actually bundled.

This verifies byte identity and provenance, not legal compliance or approval.
No files or instructions from the supplied archive are executed.
"""

import argparse
import hashlib
import json
import re
import tomllib
import zipfile
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / 'docs/verification/results/release-native-distribution-audit-2026-10-08'
SUPPLEMENT = Path('docs/verification/results/signal-hook-notice-supplement-2026-10-12')
# Independent fixed evidence; do not accept a rewritten manifest's own hashes.
SUPPLEMENT_SHA256 = 'c475a38df8004066569236a7e22aaee4a761c3d8438e23fb1fa5c5c7aa6eb9e1'
SUPPLEMENT_PACKAGES = {
    ('signal-hook', '0.3.18'): 'd881a16cf4426aa584979d30bd82cb33429027e42122b169753d6ef1085ed6e2',
    ('signal-hook-registry', '1.4.8'): 'c4db69cba1110affc0e9f7bcd48bbf87b3f4fc7c61fc9155afd4c469eb3d6c1b',
}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def source_excerpts(original, special):
    excerpts = []
    for hit in original['source_notice_hits']:
        if re.search(r'copyright', hit['text'], re.I) and not re.search(
            r'checksum|Cargo.toml|conduct', hit['path'], re.I
        ):
            excerpts.append({'path': hit['path'], 'first_line': hit['line'], 'text': hit['text']})
    for notice in special.get(original['name'], {}).get('source_notices', []):
        if notice['path'] != 'COPYRIGHT':
            for snippet in notice['snippets']:
                entry = {'path': notice['path'], **snippet, 'url': notice['url']}
                if entry not in excerpts:
                    excerpts.append(entry)
    return excerpts


def render_source_notice(name, excerpts):
    text = 'Source notice excerpts from the supplied audit.\n'
    text += 'Original license texts are retained in the adjacent files.\n'
    if name == 'chunked_transfer':
        text += 'Sean McArthur attribution is in a #[cfg(test)] unit test; source/test scope, not evidence of release binary inclusion.\n'
    if name == 'rustix':
        text += 'The vDSO header documents CC0-1.0 origin; no standalone CC0 text was supplied.\n'
        text += 'Upstream attribution reported by the audit: Written by Andrew Lutomirski, 2011-2014.\n'
    for excerpt in excerpts:
        text += f"\n--- {excerpt['path']}:{excerpt['first_line']} ---\n"
        if 'url' in excerpt:
            text += excerpt['url'] + '\n'
        text += excerpt['text']
        if not excerpt['text'].endswith('\n'):
            text += '\n'
    return text.encode('utf-8')


def external_packages(lock):
    return {(p['name'], p['version']): p['checksum'] for p in lock['package'] if 'source' in p}


def supplemental_notices(historical_zip_sha256):
    raw = (ROOT / SUPPLEMENT / 'supplement.json').read_bytes()
    assert sha(raw) == SUPPLEMENT_SHA256, 'Supplemental manifest changed'
    document = json.loads(raw)
    assert document['schema_version'] == 1
    assert document['historical_audit_zip_sha256'] == historical_zip_sha256
    assert (document['expected_historical_packages'], document['expected_current_packages'],
            document['expected_supplemental_packages']) == (42, 44, 2)
    rows = document['packages']
    assert len(rows) == 2
    assert {(r['name'], r['version']): r['checksum'] for r in rows} == SUPPLEMENT_PACKAGES
    files = set()
    for row in rows:
        identity = row['name'], row['version']
        assert row['checksum'] == row['archive_sha256'] == SUPPLEMENT_PACKAGES[identity]
        assert row['registry_source'] == 'registry+https://github.com/rust-lang/crates.io-index'
        assert row['official_archive_url'] == f"https://crates.io/api/v1/crates/{identity[0]}/{identity[1]}/download"
        assert row['adoption_status'] == 'pending_distribution_review'
        assert row['license_choice'] == 'not_selected'
        assert row['standalone_notice_files'] == row['source_copyright_hits'] == []
        assert {f['upstream_path'] for f in row['license_files']} == {'LICENSE-APACHE', 'LICENSE-MIT'}
        assert len(row['license_files']) == 2
        for entry in row['license_files']:
            expected = f"docs/third-party/licenses/{identity[0]}-{identity[1]}/{entry['upstream_path']}"
            assert entry['path'] == expected
            data = (ROOT / entry['path']).read_bytes()
            assert sha(data) == entry['sha256'] and len(data) == entry['bytes'], entry['path']
            files.add(entry['path'])
        assert {f['upstream_path'] for f in row['metadata_files']} == {'Cargo.toml', 'Cargo.toml.orig', '.cargo_vcs_info.json'}
        assert len(row['metadata_files']) == 3
        for entry in row['metadata_files']:
            expected = (SUPPLEMENT / f"{identity[0]}-{identity[1]}" / entry['upstream_path']).as_posix()
            assert entry['path'] == expected
            data = (ROOT / entry['path']).read_bytes()
            assert sha(data) == entry['sha256'] and len(data) == entry['bytes'], entry['path']
            if entry['upstream_path'].startswith('Cargo.toml'):
                metadata = tomllib.loads(data.decode())['package']
                assert (metadata['name'], metadata['version']) == identity
                assert metadata['license'] == row['declared_license_expression']
                assert metadata['repository'] == row['upstream_repository']
            else:
                assert re.fullmatch(r'[0-9a-f]{40}', json.loads(data)['git']['sha1'])
        excerpt = row['license_declaration_excerpt']
        assert excerpt['upstream_path'] == 'README.md'
        assert excerpt['path'] == (SUPPLEMENT / f"{identity[0]}-{identity[1]}" / 'README-LICENSE.txt').as_posix()
        data = (ROOT / excerpt['path']).read_bytes()
        assert sha(data) == excerpt['sha256'] and len(data) == excerpt['bytes']
        assert excerpt['end_byte'] - excerpt['start_byte'] == len(data)
        assert b'Licensed under either of' in data and b'at your option.' in data
        assert row['normalized_license_expression'] == (
            'Apache-2.0 OR MIT' if identity[0] == 'signal-hook' else 'MIT OR Apache-2.0')
    return rows, files


def check_supplement_rows(rows, ledger_text, index_text):
    for row in rows:
        prefix = f"| `{row['name']}` | `{row['version']}` |"
        ledger_rows = [r for r in ledger_text.splitlines() if r.startswith(prefix)]
        index_rows = [r for r in index_text.splitlines() if r.startswith(prefix)]
        assert len(ledger_rows) == len(index_rows) == 1, prefix
        assert row['checksum'] in ledger_rows[0]
        for text in (ledger_rows[0], index_rows[0]):
            assert row['normalized_license_expression'] in text
            for entry in row['license_files']:
                assert f"]({entry['path'].removeprefix('docs/third-party/')})" in text
            for notice in row['copyright_notices']:
                assert notice in text
        assert '要確認（配布対応）' in ledger_rows[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--guide-dir', type=Path, help='Also inspect the freshly built guide')
    parser.add_argument('--output', type=Path, help='Write a new verification report')
    args = parser.parse_args()
    manifest = json.loads((EVIDENCE / 'license-notice-manifest.json').read_text())
    archive_bytes = (EVIDENCE / manifest['audit_zip']).read_bytes()
    assert sha(archive_bytes) == manifest['audit_zip_sha256'], 'Supplied archive changed'
    with zipfile.ZipFile(EVIDENCE / manifest['audit_zip']) as archive:
        names = archive.namelist()
        assert len(names) == len(set(names)), 'Duplicate ZIP members'
        checksums = json.loads(archive.read('checksums-sha256.json'))
        assert set(names) == set(checksums) | {'checksums-sha256.json'}
        for path, checksum in checksums.items():
            assert sha(archive.read(path)) == checksum, path
        expanded = json.loads(archive.read('cargo-evidence-expanded.json'))
        summary = json.loads(archive.read('cargo-verification-summary.json'))['rows']
        special = {r['name']: r for r in json.loads(archive.read('cargo-special-license-review.json'))['results']}
        supplied_lock_bytes = archive.read('Cargo.lock')
        supplied_lock = tomllib.loads(supplied_lock_bytes.decode())
        lock_bytes = (ROOT / 'Cargo.lock').read_bytes()
        lock = tomllib.loads(lock_bytes.decode())
        locked = external_packages(lock)
        historical_locked = external_packages(supplied_lock)
        supplement, supplemental_files = supplemental_notices(manifest['audit_zip_sha256'])
        assert not set(historical_locked) & set(SUPPLEMENT_PACKAGES)
        assert locked == historical_locked | SUPPLEMENT_PACKAGES, 'Current lock differs from historical audit plus fixed supplement'
        assert len(locked) == 44
        originals = {(r['name'], r['version']): r for r in expanded}
        summaries = {(r['name'], r['version']): r for r in summary}
        mapped = {(r['name'], r['version']): r for r in manifest['cargo']}
        assert len(historical_locked) == len(expanded) == len(summary) == len(manifest['cargo']) == 42
        assert set(historical_locked) == set(originals) == set(summaries) == set(mapped)
        expected_files = set()
        original_count = 0
        notice_count = 0
        ledger_text = (ROOT / 'docs/third-party/採用物台帳.md').read_text()
        index_text = (ROOT / 'docs/third-party/第三者ライセンス表示.md').read_text()
        for identity, checksum in historical_locked.items():
            original, verified, mapping = originals[identity], summaries[identity], mapped[identity]
            assert checksum == original['checksum'] == original['actual_sha256']
            assert checksum == verified['lock_sha256'] == verified['archive_sha256'] == mapping['checksum']
            assert mapping['declared_license_expression'] == verified['declared_license_expression']
            assert mapping['normalized_license_expression'] == verified['normalized_license_expression']
            if identity[0] in special:
                assert special[identity[0]]['license_files'] == [
                    {**f, 'upstream_url': s['upstream_url']}
                    for f, s in zip(original['license_files'], special[identity[0]]['license_files'])
                ]
            assert len(mapping['license_files']) == len(original['license_files'])
            for original_file, notice_file in zip(original['license_files'], mapping['license_files']):
                data = original_file['text'].encode('utf-8')
                path = f"docs/third-party/licenses/{identity[0]}-{identity[1]}/{original_file['path']}"
                assert notice_file['path'] == path
                assert notice_file['upstream_path'] == original_file['path']
                assert sha(data) == original_file['sha256'] == notice_file['sha256']
                assert len(data) == notice_file['bytes']
                assert (ROOT / path).read_bytes() == data, path
                expected_files.add(path)
                original_count += 1
            excerpts = source_excerpts(original, special)
            assert mapping['source_excerpts'] == excerpts
            if excerpts:
                path = f"docs/third-party/licenses/{identity[0]}-{identity[1]}/SOURCE-NOTICES.txt"
                assert mapping['source_notice_path'] == path
                assert (ROOT / path).read_bytes() == render_source_notice(identity[0], excerpts), path
                expected_files.add(path)
                notice_count += 1
            else:
                assert mapping['source_notice_path'] is None
            prefix = f'| `{identity[0]}` | `{identity[1]}` |'
            ledger_rows = [r for r in ledger_text.splitlines() if r.startswith(prefix)]
            index_rows = [r for r in index_text.splitlines() if r.startswith(prefix)]
            assert len(ledger_rows) == len(index_rows) == 1, identity
            assert checksum in ledger_rows[0]
            assert mapping['normalized_license_expression'] in ledger_rows[0]
            assert mapping['normalized_license_expression'] in index_rows[0]
            for notice_file in mapping['license_files']:
                relative = notice_file['path'].removeprefix('docs/third-party/')
                assert f']({relative})' in ledger_rows[0] and f']({relative})' in index_rows[0]
            for notice in mapping['copyright_notices']:
                assert notice in ledger_rows[0] and notice in index_rows[0]
            assert '要確認（配布対応）' in ledger_rows[0]
        check_supplement_rows(supplement, ledger_text, index_text)
        expected_files.update(supplemental_files)
        original_count += sum(len(row['license_files']) for row in supplement)
        actual_files = {p.relative_to(ROOT).as_posix() for p in (ROOT / 'docs/third-party/licenses').rglob('*') if p.is_file()}
        assert actual_files == expected_files, 'Unmapped/missing bundled crate notice'
        for bad_notice in ['COPYRIGHT AND PERMISSION NOTICE', 'COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM']:
            assert bad_notice not in ledger_text and bad_notice not in index_text
        sources = (ROOT / 'docs/guide/assets/licenses/sources.txt').read_text()
        assert 'lunr-languages/master/' not in sources
        assert 'f313734d145048be2f3681b756f9bf925aa299a1' in sources
        for entry in manifest['guide']:
            original_bytes = archive.read(entry['zip_path'])
            assert sha(original_bytes) == entry['sha256']
            assert (ROOT / entry['path']).read_bytes() == original_bytes
            assert entry['url'] in sources, entry['url']
        assert len(manifest['guide']) == 8
        built_assets = []
        if args.guide_dir:
            for entry in manifest['guide']:
                path = entry['path'].removeprefix('docs/guide/')
                assert (args.guide_dir / path).read_bytes() == archive.read(entry['zip_path']), path
            page = (args.guide_dir / 'ガイドのライセンス表示.html').read_text()
            assert 'popper-2.11.8-MIT.txt' in page and 'Federico Zivolo' in page
            assert 'ガイドのライセンス表示.html' in unquote((args.guide_dir / 'index.html').read_text())
            for entry in json.loads(archive.read('published-guide/asset-comparisons.json')):
                data = (args.guide_dir / entry['path']).read_bytes()
                assert len(data) == entry['bytes'] and sha(data) == entry['sha256'], entry['path']
                built_assets.append({'path': entry['path'], 'bytes': len(data), 'sha256': sha(data)})
    report = {
        'schema_version': 1, 'generated_by': 'scripts/check_third_party_notices.py',
        'status': 'passed_originals_and_notice_correspondence',
        'verification_date': '2026-10-12', 'historical_audit_date': '2026-10-08', 'audit_zip_sha256': sha(archive_bytes),
        'supplied_payload_files_checked': len(checksums), 'locked_packages_checked': len(locked),
        'historical_locked_packages_checked': len(historical_locked),
        'supplemental_locked_packages_checked': len(supplement),
        'supplemental_manifest_sha256': SUPPLEMENT_SHA256,
        'supplemental_crate_archive_checksums': [row['archive_sha256'] for row in supplement],
        'crate_license_originals_checked': original_count, 'source_notice_bundles_checked': notice_count,
        'guide_license_originals_checked': len(manifest['guide']),
        'supplied_lock_sha256': sha(supplied_lock_bytes), 'repository_lock_sha256': sha(lock_bytes),
        'lock_package_records_equal': supplied_lock['package'] == lock['package'],
        'lock_bytes_equal': supplied_lock_bytes == lock_bytes,
        'lock_difference': 'Historical 42 registry identities/checksums unchanged; current lock adds the fixed two-crate supplement and dependency edges',
        'ledger_version': re.search(r'文書バージョン：`([^`]+)`', ledger_text).group(1),
        'ledger_sha256': sha(ledger_text.encode()),
        'fresh_guide_assets_compared': built_assets,
        'scope': 'Original byte identity, lock mapping and notice bundling; formal release archive, target runtime inclusion and adoption approval are not verified',
        'official_crate_archive_fetch': 'Historical 42: provided audit evidence; supplemental two: cached official archives matched Cargo.lock on 2026-10-12. Neither archive set is bundled or freshly fetched by this checker',
        'preserved_attachment_instructions': 'Evidence only; recommendations do not authorize actions or license choices',
    }
    if args.output:
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({k: v for k, v in report.items() if k.endswith('_checked') or k in ('status', 'lock_bytes_equal', 'ledger_version')}, ensure_ascii=False))


if __name__ == '__main__':
    main()
