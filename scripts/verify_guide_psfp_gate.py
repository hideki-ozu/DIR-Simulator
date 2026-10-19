"""Measure the two PSFP ingress-gate boundaries with a fixed formal binary."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/ethernet-psfp-gate'
COMMIT = '397900451015c9bca6a33d24feac0a88b9b7b8fe'
CASES = (76000, 76001)

def load(path):
    return json.loads(path.read_text())

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def check_inputs():
    original = load(ROOT / 'examples/ethernet/tsn/psfp-gate-model.json')
    for base in CASES:
        model = load(SOURCE / f'gate-{base}.json')
        assert model['tsn']['streams'][0]['gate']['base_time_ps'] == str(base)
        model['tsn']['streams'][0]['gate']['base_time_ps'] = '0'
        assert model == original
    a, b = [(SOURCE / f'gate-{base}.ini').read_text().replace(f'gate-{base}.json', 'MODEL.json') for base in CASES]
    assert a == b
    assert (SOURCE / 'workload.json').read_bytes() == (ROOT / 'examples/ethernet/tsn/psfp-gate-workload.json').read_bytes()
    assert (SOURCE / 'models/ethtsn/Main.ned').read_bytes() == (ROOT / 'examples/ethernet/tsn/models/ethtsn/Main.ned').read_bytes()

def measure(binary, source, output):
    output.mkdir(parents=True, exist_ok=False)
    conditions, transfer_data, commands = {}, [], []
    for base in CASES:
        name = f'gate-{base}'
        result = output / name
        for args in [('validate', '--config', str(source / (name + '.ini'))),
                     ('run', '--config', str(source / (name + '.ini')), '--output', str(result)),
                     ('view', '--input', str(result / 'results.json'), '--output', str(output / (name + '-viewer.html')))]:
            p = subprocess.run([str(binary), *args], capture_output=True, text=True)
            commands.append({'command': [str(binary), *args], 'exit_code': p.returncode, 'stdout': p.stdout, 'stderr': p.stderr})
            assert p.returncode == 0, p.stderr
        manifest = load(result / 'manifest.json')
        assert manifest['status'] == 'complete' and manifest['partial'] is False
        for f in manifest['files']:
            path = result / f['name']
            assert str(path.stat().st_size) == f['bytes'] and digest(path) == f['sha256']
        assert (result / 'diagnostics.jsonl').read_text() == ''
        doc = load(result / 'results.json'); meta, sim = doc['metadata'], doc['simulation']
        assert meta['git_commit'] == COMMIT and meta['git_dirty'] == 'false' and meta['binary_sha256'] == digest(binary)
        assert sim['termination'] == 'events_exhausted' and sim['pending_events'] == '0' and sim['partial'] is False
        assert sim['end_ps'] == '120000000'
        records = sim['model_records']
        one = lambda schema: next(r for r in records if r['schema_name'] == schema)
        tx = one('ethernet.dynamic.transfer')['data']
        rx = one('ethernet.dynamic.reception')['data']
        policing = one('ethernet.tsn.policing')
        # Independent wire arithmetic: MAC64 + preamble8 at 1Gbps; IFG12 separately.
        arrival = (64 + 8) * 8 * 10**12 // 10**9
        release = arrival + 12 * 8 * 10**12 // 10**9
        assert [tx[k] for k in ('queued_ps', 'sof_ps', 'eof_ps', 'arrival_ps', 'release_ps')] == ['0', '0', str(arrival), str(arrival), str(release)]
        assert tx['status'] == 'serialized' and tx['drop_reason'] is None
        assert tx['wire']['mac_bytes'] == '64' and tx['wire']['tag'] is None
        assert rx['observed_ps'] == rx['ready_ps'] == policing['time_ps'] == str(arrival)
        assert rx['transfer_id'] == tx['copy_id'] and policing['data']['reception_id'] == one('ethernet.dynamic.reception')['record_id']
        assert rx['ingress'] == 'Net.b.rx' and rx['vlan_id'] == '10' and rx['priority'] == '7'
        pd = policing['data']; phase = (arrival - base) % 1000000
        assert (phase < 500000) == (base == 76001)
        if base == 76000:
            assert rx['status'] == 'filtered' and rx['reason'] == pd['verdict'] == 'psfp_gate_closed'
            assert pd['color'] is None and pd['consumed_bits'] == '0'
            assert all(pd[k] is None for k in ('committed_before', 'committed_after', 'peak_before', 'peak_after'))
        else:
            assert rx['status'] == 'received' and rx['reason'] is None and pd['verdict'] == 'pass' and pd['color'] == 'green'
            assert [pd[k] for k in ('committed_before', 'peak_before', 'committed_after', 'peak_after')] == list(map(str, (512*10**12, 1024*10**12, 0, 512*10**12)))
            assert pd['consumed_bits'] == str(2 * 64 * 8)  # Sum of the two bucket consumptions.
        transfer_data.append(tx)
        conditions[name] = {'simulation': sim, 'metadata': {k: meta[k] for k in ('git_commit', 'git_dirty', 'binary_sha256', 'build_source_sha256', 'sources')},
                            'arrival_ps': arrival, 'release_ps': release, 'gate_phase_ps': phase,
                            'reception': rx, 'policing': pd, 'viewer_html_sha256': digest(output / (name + '-viewer.html'))}
    assert transfer_data[0] == transfer_data[1]
    (output / 'commands.json').write_text(json.dumps(commands, ensure_ascii=False, indent=2) + '\n')
    return conditions

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True); p.add_argument('--output', type=Path, required=True)
    p.add_argument('--publish-report', action='store_true', help='also replace the tracked measurement report after a successful run')
    a = p.parse_args(); check_inputs(); out = a.output.resolve(); out.mkdir(parents=True, exist_ok=False)
    repo = measure(a.binary.resolve(), SOURCE, out / 'repository')
    archive = ROOT / 'docs/guide/downloads/ethernet-psfp-gate-inputs.zip'
    expected = {p.relative_to(ROOT).as_posix(): p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()}
    with zipfile.ZipFile(archive) as z:
        assert set(z.namelist()) == set(expected)
        for name, contents in expected.items(): assert z.read(name) == contents
        z.extractall(out / 'isolated')
    zipped = measure(a.binary.resolve(), out / 'isolated/examples/guide/ethernet-psfp-gate', out / 'zip')
    assert {k: v['simulation'] for k, v in repo.items()} == {k: v['simulation'] for k, v in zipped.items()}
    report = {'generated_by': 'scripts/verify_guide_psfp_gate.py', 'status': 'passed', 'source_tag': 'v1.1.6', 'source_commit': COMMIT,
              'single_variable': 'tsn.streams[0].gate.base_time_ps', 'cli_operations': 12, 'zip_input_count': len(expected),
              'input_sha256': {p.relative_to(ROOT).as_posix(): digest(p) for p in SOURCE.rglob('*') if p.is_file()},
              'conditions': repo, 'zip_conditions': zipped}
    raw = json.dumps(report, ensure_ascii=False, indent=2) + '\n'
    (out / 'measurements.json').write_text(raw)
    if a.publish_report:
        dest = ROOT / 'docs/verification/results/psfp-gate-boundary'; dest.mkdir(parents=True, exist_ok=True)
        (dest / 'measurements.json').write_text(raw)
    print('PASS: PSFP arrival576000 unchanged, gate base76000/76001 closed/open, meter null/green, 12 CLI operations and isolated ZIP equality')

if __name__ == '__main__': main()
