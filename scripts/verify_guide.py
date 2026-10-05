"""Run the guide's three scenarios and verify timings, counts and manifests."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = {
    'minimal': {'a:0': [0, 238, 244], 'b:0': [244, 400, 406], 'c:0': [406, 530, 536]},
    'fast': {'a:0': [0, 119, 122], 'b:0': [122, 200, 203], 'c:0': [203, 265, 268]},
    'id-swap': {'a:0': [130, 368, 374], 'b:0': [374, 530, 536], 'c:0': [0, 124, 130]},
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'guide-evidence/recheck')
    args = parser.parse_args()
    binary = args.binary.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'commands': [], 'scenarios': {}}
    for name, expected in EXPECTED.items():
        folder = output / name
        config = ROOT / f'examples/guide/can/{name}.ini'
        for argv in [['validate', '--config', str(config)],
                     ['run', '--config', str(config), '--output', str(folder)],
                     ['view', '--input', str(folder / 'results.json'), '--output', str(output / f'{name}-viewer.html')]]:
            result = subprocess.run([str(binary), *argv], cwd=ROOT, text=True, capture_output=True)
            report['commands'].append({'argv': argv, 'exit_code': result.returncode,
                                       'stdout': result.stdout.strip(), 'stderr': result.stderr.strip()})
            assert result.returncode == 0, result.stderr
        data = json.loads((folder / 'results.json').read_text())['simulation']
        assert data['partial'] is False
        assert data['termination'] == 'events_exhausted'
        assert int(data['end_ps']) == 1_000_000_000
        assert len(data['requests']) == 3
        timings = {}
        for row in data['requests']:
            actual = [int(row['sof_ps']), int(row['eof_ps']), int(row['model_fields']['release_ps'])]
            assert actual == [int(t * 1_000_000) for t in expected[row['request_id']]]
            assert row['status'] == 'success'
            timings[row['request_id']] = actual
        assert len(data['receivers']) == 6
        assert all(row['status'] == 'received' for row in data['receivers'])
        manifest = json.loads((folder / 'manifest.json').read_text())
        for entry in manifest['files']:
            content = (folder / entry['name']).read_bytes()
            assert len(content) == int(entry['bytes'])
            assert hashlib.sha256(content).hexdigest() == entry['sha256']
        report['scenarios'][name] = {'timings_ps': timings, 'success': 3, 'received': 6,
                                      'manifest_files_verified': len(manifest['files'])}
    report['status'] = 'passed'
    (output / 'verification.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print('PASS: 3 scenarios, 9 CLI commands, 9 timing triplets, 18 receptions, 12 manifest files.')


if __name__ == '__main__':
    main()
