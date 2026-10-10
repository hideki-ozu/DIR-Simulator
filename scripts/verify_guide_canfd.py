"""Verify the synthetic CAN FD one-variable data-rate experiment and manifests."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'guide-evidence/canfd-check')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {'status': 'passed', 'variable': 'Main.bus.dataBitrate',
              'derived_changes': ['workload path', 'wire.binding_sha256'],
              'fidelity': 'synthetic precomputed phase bits; structural-only validation',
              'commands': [], 'scenarios': {}}
    fixed_ini = fixed_workload = None
    for mbps in [1, 2, 4]:
        name = f'data{mbps}'
        ini = ROOT / f'examples/guide/canfd/{name}.ini'
        fixed = [s for s in ini.read_text(encoding='utf-8').splitlines()
                 if not s.startswith(('Main.bus.dataBitrate', 'workload ='))]
        if fixed_ini is None:
            fixed_ini = fixed
        assert fixed == fixed_ini
        workload = json.loads((ini.parent / f'workload-{mbps}.json').read_text(encoding='utf-8'))
        frame = workload['generators'][0]['frame']
        expected_hash = hashlib.sha256(f'standard|0|01020304|1|30|80|500000|{mbps*1000000}'.encode()).hexdigest()
        assert frame['wire']['binding_sha256'] == expected_hash
        del frame['wire']['binding_sha256']
        if fixed_workload is None:
            fixed_workload = workload
        assert workload == fixed_workload
        folder = output / name
        commands = [['validate', '--config', str(ini)],
                    ['run', '--config', str(ini), '--output', str(folder)],
                    ['view', '--input', str(folder / 'results.json'), '--output', str(output / f'{name}-viewer.html')]]
        for argv in commands:
            result = subprocess.run([str(args.binary.resolve()), *argv], cwd=ROOT, capture_output=True, text=True)
            assert result.returncode == 0, result.stderr
            report['commands'].append({'scenario': name, 'operation': argv[0], 'exit_code': result.returncode})
        result = json.loads((folder / 'results.json').read_text(encoding='utf-8'))
        sim = result['simulation']
        assert sim['partial'] is False and sim['termination'] == 'events_exhausted'
        rows = result['model_records'] if 'model_records' in result else sim['model_records']
        requests = [r['data'] for r in rows if r['schema_name'] == 'dir.canfd.request']
        receptions = [r['data'] for r in rows if r['schema_name'] == 'dir.canfd.reception']
        duration = 60000000 + 80000000 // mbps
        sof2 = max(120000000, duration + 6000000)
        assert len(requests) == 2 and len(receptions) == 4
        for r, sof in zip(requests, [0, sof2]):
            assert r['state'] == 'serialized'
            assert [int(r[k]) for k in ['sof_ps', 'eof_ps', 'release_ps']] == [sof, sof+duration, sof+duration+6000000]
        assert all(r['state'] == 'completed' and r['arrival_ps'] == r['completed_ps'] for r in receptions)
        metrics = {r['metric']: int(r['value']) for r in sim['summary'] if r['target'] == '$all'}
        assert metrics == {'canfd.generated': 2, 'canfd.serialized': 2, 'canfd.dropped': 0, 'canfd.received': 4}
        assert {r['receiver'] for r in receptions} == {'Main.b', 'Main.c'}
        assert [int(r['generated_ps']) for r in requests] == [0, 120000000]
        assert [int(r['ready_ps']) for r in requests] == [0, 120000000]
        assert sorted(int(r['arrival_ps']) for r in receptions) == sorted([duration]*2+[sof2+duration]*2)
        frames = [r['data'] for r in rows if r['schema_name'] == 'dir.canfd.frame']
        assert len(frames) == 1 and frames[0]['wire_validation'] == 'structural-only'
        assert int(frames[0]['nominal_rate']) == 500000 and int(frames[0]['data_rate']) == mbps*1000000
        manifest = json.loads((folder / 'manifest.json').read_text(encoding='utf-8'))
        for item in manifest['files']:
            content = (folder / item['name']).read_bytes()
            assert len(content) == int(item['bytes']) and hashlib.sha256(content).hexdigest() == item['sha256']
        report['scenarios'][name] = {'data_rate_bps': mbps*1000000, 'duration_ps': duration,
                                    'requests': requests, 'receptions': receptions, 'frame': frames[0],
                                    'manifest_hashes_verified': len(manifest['files'])}
    stale = output / 'stale-binding.ini'
    content = (ROOT / 'examples/guide/canfd/data1.ini').read_text(encoding='utf-8')
    content = content.replace('ned-path = "models"', f'ned-path = "{ROOT / "examples/guide/canfd/models"}"')
    content = content.replace('model-config = "model.json"', f'model-config = "{ROOT / "examples/guide/canfd/model.json"}"')
    content = content.replace('workload = "workload-1.json"', f'workload = "{ROOT / "examples/guide/canfd/workload-1.json"}"')
    stale.write_text(content.replace('1Mbps', '2Mbps'), encoding='utf-8')
    rejected = subprocess.run([str(args.binary.resolve()), 'validate', '--config', str(stale)],
                              cwd=ROOT, capture_output=True, text=True)
    assert rejected.returncode != 0 and 'binding_sha256' in rejected.stderr
    report['stale_binding_rejected'] = True
    (output / 'verification.json').write_text(json.dumps(report, ensure_ascii=False, indent=2)+'\n', encoding='utf-8')
    print('PASS: 3 data rates, 9 CLI commands, one independent variable, phase formula/queue timings, 4 receptions per condition and manifest hashes.')


if __name__ == '__main__':
    main()
