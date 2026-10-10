"""Verify one-variable CAN receive filter examples, timing invariance and manifests."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'guide-evidence/filter-check')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {'scenarios': {}, 'commands': []}
    configs = {name: (ROOT / f'examples/guide/can/{name}.ini').read_text(encoding='utf-8')
               for name in ['minimal', 'filter-id', 'filter-none']}
    baseline = [line for line in configs['minimal'].splitlines() if line]
    for name in ['filter-id', 'filter-none']:
        assert [line for line in configs[name].splitlines() if line and not line.startswith('Main.c.rxFilter')] == baseline
    for name, expected_c in [('minimal', ['received', 'received']),
                             ('filter-id', ['received', 'filtered']),
                             ('filter-none', ['filtered', 'filtered'])]:
        folder = output / name
        argv_list = [['validate', '--config', f'examples/guide/can/{name}.ini'],
                     ['run', '--config', f'examples/guide/can/{name}.ini', '--output', str(folder)],
                     ['view', '--input', str(folder / 'results.json'), '--output', str(output / f'{name}-viewer.html')]]
        for argv in argv_list:
            result = subprocess.run([str(args.binary.resolve()), *argv], cwd=ROOT, capture_output=True, text=True)
            assert result.returncode == 0, result.stderr
            report['commands'].append({'operation': argv[0], 'scenario': name, 'exit_code': result.returncode})
        d = json.loads((folder / 'results.json').read_text(encoding='utf-8'))['simulation']
        assert d['partial'] is False and d['termination'] == 'events_exhausted'
        timings = {}
        for row in d['requests']:
            assert row['status'] == 'success'
            timings[row['request_id']] = [int(row['sof_ps']), int(row['eof_ps']), int(row['model_fields']['release_ps'])]
        assert timings == {'a:0': [0, 238000000, 244000000], 'b:0': [244000000, 400000000, 406000000], 'c:0': [406000000, 530000000, 536000000]}
        assert len(d['receivers']) == 6
        c_rows = [r for r in d['receivers'] if r['receiver'] == 'Main.c']
        assert [r['status'] for r in c_rows] == expected_c
        assert [r['request_id'] for r in c_rows] == ['a:0', 'b:0']
        for row in d['receivers']:
            if row['receiver'] != 'Main.c':
                assert row['status'] == 'received'
            assert row['observed_ps'] is not None
            assert (row['received_ps'] is None) == (row['status'] == 'filtered')
            if row['status'] == 'received':
                assert row['received_ps'] == row['observed_ps']
        manifest = json.loads((folder / 'manifest.json').read_text(encoding='utf-8'))
        for entry in manifest['files']:
            contents = (folder / entry['name']).read_bytes()
            assert len(contents) == int(entry['bytes'])
            assert hashlib.sha256(contents).hexdigest() == entry['sha256']
        report['scenarios'][name] = {'success': len(d['requests']),
                                   'received': sum(r['status'] == 'received' for r in d['receivers']),
                                   'filtered': sum(r['status'] == 'filtered' for r in d['receivers']),
                                   'c_received': sum(r['status'] == 'received' for r in c_rows),
                                   'timings_ps': timings, 'receivers': d['receivers'],
                                   'manifest_files_verified': len(manifest['files'])}
    report['status'] = 'passed'
    report['one_changed_setting'] = 'Main.c.rxFilter'
    (output / 'verification.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print('PASS: rxFilter is the only changed setting; 3 scenarios, 9 CLI commands, invariant TX timings, exact receiver states, 12 manifest hashes.')


if __name__ == '__main__':
    main()
