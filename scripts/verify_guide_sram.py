"""Verify SRAM port comparisons and isolated downloadable inputs with a supplied CLI."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/sram-ports'

def measure(binary, source, output):
    output.mkdir(parents=True)
    values, provenance = {}, {}
    for ports in (1, 2, 3):
        name = f'ports{ports}'
        result = output / name
        for args in [
            ['validate', '--config', str(source / (name + '.ini'))],
            ['run', '--config', str(source / (name + '.ini')), '--output', str(result)],
            ['view', '--input', str(result / 'results.json'), '--output', str(output / (name + '-viewer.html'))],
        ]:
            run = subprocess.run([str(binary), *args], capture_output=True, text=True)
            assert run.returncode == 0, run.stderr
            parsed = json.loads(run.stdout)
            assert parsed.get('status') in ('valid', 'complete') or parsed.get('exit_code') == 0
        manifest = json.loads((result / 'manifest.json').read_text())
        assert manifest['status'] == 'complete' and not manifest['partial']
        for entry in manifest['files']:
            raw = (result / entry['name']).read_bytes()
            assert str(len(raw)) == entry['bytes']
            assert hashlib.sha256(raw).hexdigest() == entry['sha256']
        document = json.loads((result / 'results.json').read_text())
        meta, sim = document['metadata'], document['simulation']
        assert meta['git_commit'] == 'b45515644dfc65c205216d564d5bf7ef00280622'
        assert meta['git_dirty'] in (False, 'false', 'clean')
        assert not sim['partial'] and sim['termination'] == 'events_exhausted'
        assert sim['end_ps'] == '500'
        requests = [r for r in sim['model_records'] if r['schema_name'] == 'memory-ipc.request']
        assert len(requests) == 3
        for i, r in enumerate(sorted(requests, key=lambda x: x['request_id'])):
            d = r['data']
            assert r['request_id'] == 'abc'[i] + ':0'
            assert d['generated_ps'] == '0'
            assert d['started_ps'] == str((i // ports) * 100)
            assert d['completed_ps'] == str((i // ports + 1) * 100)
            assert d['status'] == 'completed' and d['reason'] == 'ok'
            assert d['output_hex'] == ['01020304', '05060708', '090a0b0c'][i]
        memory = [r for r in sim['model_records'] if r['schema_name'] == 'memory-ipc.memory']
        assert memory[-1]['data']['hex'] == '0102030405060708090a0b0c0d0e0f10'
        summary = {r['metric']: r['value'] for r in sim['summary']}
        assert summary['memory_ipc.offered'] == summary['memory_ipc.completed'] == '3'
        assert summary['memory_ipc.rejected'] == '0'
        values[name] = sim
        provenance[name] = {k: meta[k] for k in ('git_commit','git_dirty','binary_sha256','build_source_sha256','cargo_lock_sha256')}
        provenance[name]['input_sha256'] = {s['logical_path']: s['sha256'] for s in meta['sources']}
    return values, provenance

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--output', type=Path, default=ROOT / 'guide-evidence/sram-verified')
    args = p.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    base = json.loads((SOURCE / 'ports1.json').read_text())
    base['sram'][0].pop('ports')
    for ports in (1,2,3):
        name = f'ports{ports}'
        value = json.loads((SOURCE / (name + '.json')).read_text())
        assert value['sram'][0].pop('ports') == ports
        assert value == base
        assert (SOURCE / (name + '.ini')).read_text().replace(name+'.json','MODEL.json') == (SOURCE/'ports1.ini').read_text().replace('ports1.json','MODEL.json')
    public, provenance = measure(args.binary.resolve(), SOURCE, output/'repository')
    with zipfile.ZipFile(ROOT/'docs/guide/downloads/sram-ports-inputs.zip') as z:
        expected = {p.relative_to(ROOT).as_posix():p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()}
        assert set(z.namelist()) == set(expected)
        for name, raw in expected.items():
            assert z.read(name) == raw
        z.extractall(output/'isolated')
    zipped, zip_provenance = measure(args.binary.resolve(), output/'isolated/examples/guide/sram-ports',output/'zip')
    assert public == zipped
    evidence = {'status':'passed','source_tag':'v1.1.4','source_commit':'b45515644dfc65c205216d564d5bf7ef00280622','cli_operations':18,'zip_input_count':len(expected),'single_variable':'sram[0].ports','zip_reproduction':'all simulation objects identical','conditions':public,'provenance':provenance,'zip_provenance':zip_provenance}
    destination = ROOT/'docs/verification/results/sram-ports'
    destination.mkdir(parents=True,exist_ok=True)
    (destination/'measurements.json').write_text(json.dumps(evidence,ensure_ascii=False,indent=2)+'\n')
    print('PASS: 18 CLI operations, three port conditions, hashes and isolated ZIP reproduction')

if __name__ == '__main__':
    main()
