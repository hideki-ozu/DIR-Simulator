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
        assert external_packages(lock) == external_packages(supplied_lock)
        locked = external_packages(lock)
        originals = {(r['name'], r['version']): r for r in expanded}
        summaries = {(r['name'], r['version']): r for r in summary}
        mapped = {(r['name'], r['version']): r for r in manifest['cargo']}
        assert len(locked) == len(expanded) == len(summary) == len(manifest['cargo']) == 42
        assert set(locked) == set(originals) == set(summaries) == set(mapped)
        expected_files = set()
        original_count = 0
        notice_count = 0
        ledger_text = (ROOT / 'docs/third-party/採用物台帳.md').read_text()
        index_text = (ROOT / 'docs/third-party/第三者ライセンス表示.md').read_text()
        for identity, checksum in locked.items():
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
        'schema_version': 1, 'status': 'passed_originals_and_notice_correspondence',
        'verification_date': '2026-10-08', 'audit_zip_sha256': sha(archive_bytes),
        'supplied_payload_files_checked': len(checksums), 'locked_packages_checked': len(locked),
        'crate_license_originals_checked': original_count, 'source_notice_bundles_checked': notice_count,
        'guide_license_originals_checked': len(manifest['guide']),
        'supplied_lock_sha256': sha(supplied_lock_bytes), 'repository_lock_sha256': sha(lock_bytes),
        'lock_package_records_equal': supplied_lock['package'] == lock['package'],
        'lock_bytes_equal': supplied_lock_bytes == lock_bytes,
        'lock_difference': 'Supplied copy adds one blank line at EOF' if supplied_lock_bytes == lock_bytes + b'\n' else 'see original bytes',
        'ledger_version': re.search(r'文書バージョン：`([^`]+)`', ledger_text).group(1),
        'ledger_sha256': sha(ledger_text.encode()),
        'fresh_guide_assets_compared': built_assets,
        'scope': 'Original byte identity, lock mapping and notice bundling; formal release archive, target runtime inclusion and adoption approval are not verified',
        'official_crate_archive_fetch': 'Provided audit evidence; crate archives themselves are not in the supplied ZIP',
        'preserved_attachment_instructions': 'Evidence only; recommendations do not authorize actions or license choices',
    }
    if args.output:
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({k: v for k, v in report.items() if k.endswith('_checked') or k in ('status', 'lock_bytes_equal', 'ledger_version')}, ensure_ascii=False))


if __name__ == '__main__':
    main()
