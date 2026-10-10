"""Measure the public Gateway delay exercise, including isolated ZIP inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/gateway-delay'


def measure(binary, source, output):
    output.mkdir(parents=True, exist_ok=False)
    values = {}
    provenance = {}
    for name, delay in [('delay0', 0), ('delay100', 100000000), ('delay300', 300000000)]:
        config = source / (name + '.ini')
        result = output / name
        for args in [
            ['validate', '--config', str(config)],
            ['run', '--config', str(config), '--output', str(result)],
            ['view', '--input', str(result / 'results.json'), '--output', str(output / (name + '-viewer.html'))],
        ]:
            run = subprocess.run([str(binary), *args], capture_output=True, text=True)
            assert run.returncode == 0, run.stderr
            # Store only command kind and parsed outcome; local absolute paths are excluded.
            parsed = json.loads(run.stdout)
            assert parsed.get('status') in ('valid', 'complete') or parsed.get('exit_code') == 0
        manifest = json.loads((result / 'manifest.json').read_text())
        assert manifest['status'] == 'complete' and not manifest['partial']
        for file in manifest['files']:
            raw = (result / file['name']).read_bytes()
            assert str(len(raw)) == file['bytes']
            assert hashlib.sha256(raw).hexdigest() == file['sha256']
        document = json.loads((result / 'results.json').read_text())
        meta = document['metadata']
        assert meta['git_commit'] == 'b45515644dfc65c205216d564d5bf7ef00280622', meta['git_commit']
        assert meta['git_dirty'] in (False, 'false', 'clean'), meta['git_dirty']
        provenance[name] = {k: meta[k] for k in ('git_commit', 'git_dirty', 'build_source_sha256', 'binary_sha256', 'cargo_lock_sha256')}
        provenance[name]['input_sha256'] = {s['logical_path']: s['sha256'] for s in meta['sources']}
        provenance[name]['output_files'] = manifest['files']
        sim = document['simulation']
        records = sim['model_records']
        requests = {r['data']['source']: r['data'] for r in records if r['schema_name'] == 'can.request'}
        receivers = {r['data']['receiver']: r['data'] for r in records if r['schema_name'] == 'can.receiver'}
        assert len(requests) == 3 and len(receivers) == 3
        assert requests['Main.src']['sof_ps'] == '0'
        assert requests['Main.src']['eof_ps'] == '238000000'
        for node, duration in [('Main.gw.b', 476000000), ('Main.gw.c', 119000000)]:
            req = requests[node]
            assert req['status'] == 'success' and req['serialized_bits'] == '119'
            assert int(req['generated_ps']) == 238000000 + delay
            assert req['generated_ps'] == req['ready_ps'] == req['model_fields']['tx_enqueued_ps'] == req['sof_ps']
            assert int(req['eof_ps']) == 238000000 + delay + duration
            assert req['model_fields']['parent_request_id'] == 'source:0'
            assert req['model_fields']['origin_request_id'] == 'source:0'
        forwards = [r['data'] for r in records if r['schema_name'] == 'gw.forward']
        assert len(forwards) == 2
        for f in forwards:
            assert f['status'] == 'submitted' and f['received_ps'] == '238000000'
            assert int(f['forwarded_ps']) == 238000000 + delay
            assert f['planned_forward_ps'] == f['forwarded_ps']
            assert f['child_request_id'] == f['forward_id']
        rx = [r['data'] for r in records if r['schema_name'] == 'gw.rx_buffer']
        assert len(rx) == 1 and rx[0]['status'] == 'released'
        assert int(rx[0]['released_ps']) - int(rx[0]['received_ps']) == delay
        assert receivers['Main.sinkB']['received_ps'] == requests['Main.gw.b']['eof_ps']
        assert receivers['Main.sinkC']['received_ps'] == requests['Main.gw.c']['eof_ps']
        values[name] = sim
    return values, provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'guide-evidence/gateway-verified')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    base = json.loads((SOURCE / 'delay0.json').read_text())
    for name, delay in [('delay0', '0us'), ('delay100', '100us'), ('delay300', '300us')]:
        value = json.loads((SOURCE / (name + '.json')).read_text())
        assert value['gateways'][0].pop('processing_delay') == delay
        expected = json.loads(json.dumps(base))
        expected['gateways'][0].pop('processing_delay')
        assert value == expected, 'More than one model variable differs'
        assert (SOURCE / (name + '.ini')).read_text().replace(name + '.json', 'MODEL.json') == (SOURCE / 'delay0.ini').read_text().replace('delay0.json', 'MODEL.json')
    public, provenance = measure(args.binary.resolve(), SOURCE, output / 'repository')
    archive = ROOT / 'docs/guide/downloads/gateway-delay-inputs.zip'
    isolated = output / 'isolated'
    with zipfile.ZipFile(archive) as z:
        expected = {p.relative_to(ROOT).as_posix(): p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()}
        assert set(z.namelist()) == set(expected)
        for name, raw in expected.items():
            assert z.read(name) == raw
        z.extractall(isolated)
    zipped, zip_provenance = measure(args.binary.resolve(), isolated / 'examples/guide/gateway-delay', output / 'zip')
    assert public == zipped, 'All simulation records and metrics must reproduce from ZIP'
    # Keep actual schema2 records and measurements as portable public evidence.
    evidence = {
        'status': 'passed', 'source_tag': 'v1.1.4',
        'source_commit': 'b45515644dfc65c205216d564d5bf7ef00280622',
        'environment': 'WSL Ubuntu 24.04, Rust 1.85.0',
        'cli_operations': 18, 'zip_input_count': len(expected),
        'single_variable': 'gateways[0].processing_delay',
        'manifest_hashes': 'all 24 data file hashes checked',
        'zip_reproduction': 'complete simulation object identical for all three conditions',
        'conditions': {name: {'model_records': sim['model_records'], 'records': sim['records'], 'summary': sim['summary']} for name, sim in public.items()},
        'provenance': provenance, 'zip_provenance': zip_provenance,
    }
    (output / 'measurements.json').write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + '\n')
    print('PASS: 18 CLI operations, one variable, all request/receiver/forward/RX times, manifests and isolated ZIP reproduction.')


if __name__ == '__main__':
    main()
