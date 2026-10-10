"""Reconcile saved populations without rebuilding, approving adoption, or rewriting old packets."""
import argparse
import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'docs/verification/results/ac0008-origin-followup-2026-10-10'


def read(path):
    return json.loads(path.read_text(encoding='utf-8'))


def function_body(text, marker):
    """Extract these pinned function bodies; no JavaScript is executed."""
    start = text.index('{', text.index(marker))
    depth = 0
    quote = comment = None
    escaped = False
    i = start
    while i < len(text):
        char, pair = text[i], text[i:i+2]
        if comment == 'line':
            if char == '\n': comment = None
        elif comment == 'block':
            if pair == '*/': comment = None; i += 1
        elif quote:
            if escaped: escaped = False
            elif char == '\\': escaped = True
            elif char == quote: quote = None
        elif pair == '//': comment = 'line'; i += 1
        elif pair == '/*': comment = 'block'; i += 1
        elif char in "'\"`": quote = char
        elif char == '{': depth += 1
        elif char == '}':
            depth -= 1
            if depth == 0: return text[start:i+1]
        i += 1
    raise ValueError('Unterminated pinned function body')


def check_upstream(cache):
    receipts = read(OUT/'source-receipts.json')
    items = {x['index']: x for x in receipts['sources']}
    texts = {}
    for index in range(8, 14):
        data = (cache/('input-'+str(index)+'.txt')).read_bytes()
        assert hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest() == items[index]['git_blob']
        assert hashlib.sha256(data).hexdigest() == items[index]['sha256']
        texts[index] = data.decode('utf-8')
    left = re.sub(r'\s+', '', function_body(texts[10], 'function SnowballProgram()'))
    right = re.sub(r'\s+', '', function_body(texts[11], 'SnowballProgram: function()'))
    assert left == right and len(left) == 3743
    assert hashlib.sha256(left.encode()).hexdigest() == receipts['comparison']['normalized_utf8_sha256']
    assert 'MOZILLA PUBLIC LICENSE' in texts[8].upper() and '1.1' in texts[8]
    assert 'Among.prototype.toCharArray' in texts[9] and 'this.toCharArray = function' in texts[11]
    assert re.search(r'function\(word\)\s*\{\s*return word;\s*\}', texts[12])
    assert re.search(r"\{\s*locale: 'ja'\s*\}", texts[13])
    print('PASS: pinned upstream bytes and 3743-character body comparison; no JavaScript execution')


def build():
    receipts = read(OUT / 'source-receipts.json')
    sources = receipts['sources']
    indexed = {x['index']: x for x in sources}
    for index in (0, 1, 2, 3, 5, 6, 14):
        item = indexed[index]
        data = (ROOT / item['path']).read_bytes()
        digest = hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()
        assert digest == item['git_blob'], ('historical input changed', item['path'])
    graph, artifacts, python, elf, ledger = [read(ROOT / indexed[i]['path']) for i in (0, 1, 2, 3, 5)]
    identification = read(ROOT / indexed[14]['path'])
    assert identification['source_commit'] == 'ddfa7bd916212a0b9fedb6adffa9bdb5e83c8401'
    metadata = {n['name'] + '@' + n['version'] for n in graph['nodes'] if n['id'].startswith('registry+')}
    package_ids = {x['package_id'] for x in artifacts}
    observed = {x.split('#', 1)[1] for x in package_ids if x.startswith('registry+')}
    assert len(metadata) == 30 and len(observed) == 28
    assert metadata - observed == {'errno@0.3.14', 'libc@0.2.189'}
    assert observed <= metadata and len(artifacts) == 42 and len(package_ids) == 29
    assert len(python['current_wheels']) == len(python['current_installed_distributions']) == 17
    assert python['same_17_name_version_pairs_as_prior']
    for wheel in python['current_wheels']:
        assert wheel['matches_prior']
        assert wheel['recorded']['sha256'] == wheel['current']['sha256']
        assert wheel['recorded']['bytes'] == wheel['current']['bytes']
    assert not python['archive_wheel_members']
    assert not python['generator_environment_rebuilt'] and not python['guide_rebuilt']
    assets = python['fixed_artifact_guide_asset_checks']
    assert len(assets) == 27 and len({x['guide_relative_path'] for x in assets}) == 27
    for item in assets:
        assert item['all_hashes_match']
        assert item['archive_members'] and item['wheel_members']
        assert all(x['sha256'] == item['recorded_sha256'] for x in item['archive_members'] + item['wheel_members'])
    fixed = elf['artifacts'][0]
    assert fixed['source_commit'] == '0b7b23d8e23fcb1a1491cd13cd42aa5b96a9ab1c'
    assert fixed['binary']['sha256'] == 'd29b180dd7b46a46a52a18bf62ba3f5628d87e3303efaf398e0bf09c7a3e7129'
    assert fixed['binary']['bytes'] == 11637648
    assert fixed['interpreter'] == '/lib64/ld-linux-x86-64.so.2'
    assert fixed['dt_needed'] == ['libgcc_s.so.1', 'libc.so.6', 'ld-linux-x86-64.so.2']
    assert len(ledger['rows']) == 50 and not ledger['bulk_approval']
    assert all(x['approval'] == 'unapproved' and x['proposed_route'] is None for x in ledger['rows'])
    assert sum(' OR ' in x['route_options'] for x in ledger['rows']) == 40
    assert receipts['gitlink']['mode'] == '160000'
    assert receipts['gitlink']['sha'] == receipts['fork']['ref'] == 'f7cdf98e5be76f77f64ecf4cf17acc2d907f6e60'
    assert indexed[8]['git_blob'] == 'a21bd959dfe694cf39a5b0fcd51fcff1790cdab8'
    comparison = receipts['comparison']
    assert comparison['normalized_characters'] == 3743
    assert comparison['normalized_utf8_sha256'] == '45a3fca22291091557083dde3fcc71f2d308ac19f8772a3d9c141e3c5b563bd3'
    assert not comparison['file_byte_identity'] and not comparison['execution_equivalence_tested']
    for index, asset in ((11, 'search/lunr.stemmer.support.js'), (12, 'search/lunr.ja.js')):
        assert indexed[index]['sha256'] == next(x['recorded_sha256'] for x in assets if x['guide_relative_path'] == asset)
    rows = []
    for row in ledger['rows']:
        name = row['key'].removeprefix('cargo:')
        scope = ('observed_compiler_artifact' if name in observed else
                 'metadata_only' if name in metadata else 'absent_from_this_linux_metadata') if row['kind'] == 'Cargo' else 'guide_asset_population_separate'
        rows.append(dict(key=row['key'], kind=row['kind'], scope=scope, approval='unapproved',
                         route_selected=None, complete_static_membership='unknown',
                         original_archive_membership_retained=True,
                         row_specific_archive_membership_not_newly_approved=True))
    assert sum(x['scope'] == 'absent_from_this_linux_metadata' for x in rows) == 12
    umd = ledger['additional_subcomponents'][0]
    assert umd['approval'] == 'unapproved' and not umd['byte_identity_claim']
    return dict(schema_version=1, document_version='1.1.0', document_id='ac0008-origin-followup',
                document_history=[dict(version='1.1.0', date='2026-10-10', change='Initial scoped reconciliation of saved evidence and pinned upstream textual comparison')],
                saved_evidence_commit='b45515644dfc65c205216d564d5bf7ef00280622',
                metadata=dict(source_commit=graph['source_commit'], target=graph['target'], external_packages=sorted(metadata)),
                compiler=dict(source_commit=identification['source_commit'], artifact_records=42, package_ids_including_product=29, external_packages=sorted(observed)),
                python_environment=dict(installed_distributions=python['current_installed_distributions'], wheel_count=17, wheel_members_in_fixed_archive=0, rebuilt=False),
                guide=dict(assets=assets, checked_members=27, other_guide_members_not_reaudited=37),
                fixed_ELF=dict(source_commit=fixed['source_commit'], archive=fixed['origin']['archive'], binary=fixed['binary'], member=fixed['origin']['member'], interpreter=fixed['interpreter'], dt_needed=fixed['dt_needed'], complete_static_object_members='unknown', historical_host_library_bytes='unknown'),
                ledger_scoped_rows=rows, approvals=dict(adoption_unapproved=50, OR_unselected=40, author_attestation='not received in this work'),
                UMD=dict(existing_origin_fields_preserved=True, approval=umd['approval'], byte_identity_claim=False, remaining=['Exact import revision', 'Final notice/source-list integration and adoption/author decision']),
                Snowball=dict(receipt='source-receipts.json', comparison=receipts['comparison'], original_Urim_archive='unconfirmed', initial_import_history='unconfirmed', adoption='unapproved', modern_BSD_substitution=False),
                source_receipts='source-receipts.json', historical_packet_generator_unchanged=True,
                rustix_CC0='Earlier HTTP403 remains unresolved; no retrieval/retry/alternate route',
                Library_original_JSON_received=False, new_build=False, new_artifact_audit=False, product_execution=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--upstream-cache', type=Path, help='Optional six previously downloaded pinned input-8..13.txt files; no network requests')
    args = parser.parse_args()
    if args.upstream_cache:
        check_upstream(args.upstream_cache)
    rendered = json.dumps(build(), ensure_ascii=False, indent=2) + '\n'
    target = OUT / 'scoped-reconciliation.json'
    if args.check:
        assert target.read_text(encoding='utf-8') == rendered, 'stale scoped reconciliation'
    else:
        target.write_text(rendered, encoding='utf-8', newline='\n')
    print('PASS: 30 metadata / 28 observed / 12 other ledger packages; 17 wheels / 27 assets; 50 unapproved / 40 OR unselected; no new build/audit')


if __name__ == '__main__':
    main()
