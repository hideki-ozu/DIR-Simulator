"""Aggregate completed local guide checks into the repository's evidence report."""
import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE = ROOT / 'guide-evidence'


def read(name):
    return json.loads((EVIDENCE / name).read_text(encoding='utf-8'))


def main():
    report = read('recheck/verification.json')
    prefixes = set()
    for command in report['commands']:
        for arg in command['argv']:
            if 'examples/guide/can/' in arg:
                prefixes.add(arg.split('examples/guide/can/')[0])
    serialized = json.dumps(report, ensure_ascii=False)
    for prefix in prefixes:
        if prefix:
            serialized = serialized.replace(prefix, '')
    report = json.loads(serialized)
    report['browser'] = read('browser-verification.json')
    report['links'] = read('link-verification.json')
    report['existing_tests'] = {}
    for name, log in [('rust_runtime_can', 'rust-can-tests.log'),
                      ('rust_can_scenarios', 'rust-can-scenarios.log')]:
        text = (EVIDENCE / log).read_text(encoding='utf-8')
        report['existing_tests'][name] = {
            'passed': sum(map(int, re.findall(r'(\d+) passed', text))),
            'failed': sum(map(int, re.findall(r'(\d+) failed', text))),
        }
    text = (EVIDENCE / 'node-viewer-tests.log').read_text(encoding='utf-8')
    report['existing_tests']['node_viewer_model'] = {
        'passed': int(re.search(r'\bpass (\d+)', text)[1]),
        'failed': int(re.search(r'\bfail (\d+)', text)[1]),
    }
    assert all(t['failed'] == 0 and t['passed'] > 0 for t in report['existing_tests'].values())
    report['build'] = {'mkdocs': '1.6.1', 'strict': True, 'search_language': ['ja'],
                       'minimum_search_length': 2, 'site_prefix': '/guide/'}
    report['release_reference'] = 'https://github.com/hideki-ozu/DIR-Simulator/releases/tag/v1.1.3'
    report['scope'] = {'remote_writes': False, 'qiita_posting': False, 'deployment': False,
                       'deleted_comparison_artifacts_used': False, 'windows_native_rust_tested': False}
    report['screenshots'] = {
        p.relative_to(ROOT).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in (ROOT / 'docs/guide/assets').glob('*.png')
    }
    report['generated_by'] = 'scripts/record_guide_verification.py'
    destination = ROOT / 'docs/verification/results/guide-local-verification.json'
    destination.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print('Recorded completed local checks and screenshot hashes; absolute host paths removed.')


if __name__ == '__main__':
    main()
