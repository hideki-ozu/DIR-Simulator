"""Measure static multicast egress choices with formal CLI and isolated ZIP."""
import argparse, hashlib, json, subprocess, zipfile, zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/ethernet-multicast'
COMMIT = 'b45515644dfc65c205216d564d5bf7ef00280622'
CASES = {'none': [], 'local': ['VlanDemo.s1.tx_c'],
         'branch': ['VlanDemo.s1.tx_b', 'VlanDemo.s1.tx_c']}


def check_inputs():
    baseline = json.loads((SOURCE / 'branch.json').read_text())
    assert baseline == json.loads((ROOT / 'examples/ethernet/model-vlan.json').read_text())
    baseline['switches'][0]['multicast'][0]['egresses'] = []
    for case, egresses in CASES.items():
        model = json.loads((SOURCE / (case + '.json')).read_text())
        assert model['switches'][0]['multicast'][0]['egresses'] == egresses
        model['switches'][0]['multicast'][0]['egresses'] = []
        assert model == baseline, 'Comparison changed more than one egress list'
        ini = (SOURCE / (case + '.ini')).read_text().replace(case + '.json', 'MODEL.json')
        assert ini == (SOURCE / 'branch.ini').read_text().replace('branch.json', 'MODEL.json')
    for name in ['Main.ned', 'VlanDemo.ned']:
        assert (SOURCE / 'models/ethdemo' / name).read_bytes() == (ROOT / 'examples/ethernet/models/ethdemo' / name).read_bytes()
    assert (SOURCE / 'workload.json').read_bytes() == (ROOT / 'examples/ethernet/vlan-multicast.json').read_bytes()


def measure(binary, source, output):
    output.mkdir(parents=True)
    simulations, provenance = {}, {}
    for case in CASES:
        result = output / case
        for args in [ ['validate', '--config', str(source / (case + '.ini'))],
                     ['run', '--config', str(source / (case + '.ini')), '--output', str(result)],
                     ['view', '--input', str(result / 'results.json'), '--output', str(output / (case + '-viewer.html'))] ]:
            proc = subprocess.run([str(binary), *args], capture_output=True, text=True)
            assert proc.returncode == 0, proc.stderr
        manifest = json.loads((result / 'manifest.json').read_text())
        assert manifest['status'] == 'complete' and not manifest['partial']
        for entry in manifest['files']:
            raw = (result / entry['name']).read_bytes()
            assert hashlib.sha256(raw).hexdigest() == entry['sha256'] and str(len(raw)) == entry['bytes']
        doc = json.loads((result / 'results.json').read_text())
        meta, sim = doc['metadata'], doc['simulation']
        assert meta['git_commit'] == COMMIT and meta['git_dirty'] in (False, 'false', 'clean'), (meta['git_commit'], meta['git_dirty'])
        assert sim['termination'] == 'events_exhausted' and not sim['partial'] and sim['pending_events'] == '0'
        rows = sim['model_records']
        group = lambda name: [r for r in rows if r['schema_name'] == name]
        frames, transfers, receptions = [group('ethernet.' + name) for name in ['frame', 'transfer', 'reception']]
        assert len(frames) == 1 and frames[0]['data']['dst_mac'] == '01:00:5e:00:00:01'
        assert frames[0]['data']['source_vlan_id'] == '20'
        expected_outputs = {'none': ['a.tx'], 'local': ['a.tx', 's1.tx_c'],
                            'branch': ['a.tx', 's1.tx_b', 's1.tx_c', 's2.tx_b', 's2.tx_c', 's3.tx_b', 's3.tx_c']}[case]
        assert sorted(t['data']['from_port'] for t in transfers) == sorted('VlanDemo.' + x for x in expected_outputs)
        assert len(receptions) == len(transfers)
        timing = {'a.tx': (0, 609000), 's1.tx_b': (611000, 1220000), 's1.tx_c': (611000, 1220000),
                  's2.tx_b': (1222000, 1831000), 's2.tx_c': (1222000, 1831000),
                  's3.tx_b': (1833000, 2442000), 's3.tx_c': (1833000, 2442000)}
        by_id = {t['record_id']: t for t in transfers}
        for t in transfers:
            assert t['schema_version'] == 3
            d = t['data']; sof, arrival = timing[d['from_port'].removeprefix('VlanDemo.')]
            assert d['status'] == 'serialized' and d['drop_reason'] is None
            assert d['sof_ps'] == str(sof) and d['eof_ps'] == str(sof + 608000)
            assert d['release_ps'] == str(sof + 704000) and d['arrival_ps'] == str(arrival)
            assert d['vlan_id'] == '20' and d['priority'] == '4'
            wire = d['wire']; raw = bytes.fromhex(wire['mac_hex'])
            assert wire['data_hex'] == '010203' and wire['mac_bytes'] == '68' and wire['pad_bytes'] == '43'
            assert wire['tag'] == {'vid': '20', 'pcp': '4', 'dei': '1'}
            assert len(raw) == 68 and raw[-4:] == zlib.crc32(raw[:-4]).to_bytes(4, 'little')
            if d['parent_transfer_id'] is None:
                assert d['from_port'] == 'VlanDemo.a.tx'
            else:
                parent = by_id[d['parent_transfer_id']]['data']
                assert int(parent['arrival_ps']) + 2000 == sof
        for r in receptions:
            assert r['schema_version'] == 2
            d = r['data']; incoming = by_id[d['transfer_id']]['data']
            assert d['observed_ps'] == incoming['arrival_ps']
            children = [t['record_id'] for t in transfers if t['data']['parent_transfer_id'] == d['transfer_id']]
            assert sorted(children) == sorted(d['egress_transfer_ids'])
            if r['subject'] == 'VlanDemo.s1' and case == 'none':
                assert d['status'] == 'filtered' and d['reason'] == 'multicast_no_egress' and d['ready_ps'] == '611000'
            elif r['subject'] == 'VlanDemo.c':
                assert d['status'] == 'received' and d['reason'] is None and d['ready_ps'] == '1220000'
            elif r['subject'] in ['VlanDemo.b', 'VlanDemo.d', 'VlanDemo.e']:
                assert d['status'] == 'filtered' and d['reason'] == 'multicast_not_subscribed'
            else:
                assert d['status'] == 'forwarded' and d['reason'] is None
        summary = {(r['target'], r['metric']): r['value'] for r in sim['summary']}
        received, filtered, forwarded = {'none': (0, 1, 0), 'local': (1, 0, 1), 'branch': (1, 3, 3)}[case]
        for metric, expected in {'generated': 1, 'transfer_offered': len(transfers), 'serialized': len(transfers),
                                 'received': received, 'filtered': filtered, 'forwarded': forwarded,
                                 'dropped': 0, 'queued': 0, 'transmitting': 0, 'processing': 0}.items():
            assert summary.get(('$all', 'ethernet.' + metric), '0') == str(expected), (metric, summary)
        assert summary.get(('$all', 'ethernet.filtered.multicast_no_egress'), '0') == ('1' if case == 'none' else '0')
        assert summary.get(('$all', 'ethernet.filtered.multicast_not_subscribed'), '0') == ('3' if case == 'branch' else '0')
        simulations[case] = sim
        provenance[case] = {k: meta[k] for k in ['git_commit', 'git_dirty', 'compiler', 'target_triple', 'binary_sha256', 'build_source_sha256', 'cargo_lock_sha256']}
        provenance[case]['input_sha256'] = {s['logical_path']: s['sha256'] for s in meta['sources']}
    return simulations, provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args(); check_inputs()
    out = args.output.resolve(); out.mkdir(parents=True, exist_ok=False)
    public, provenance = measure(args.binary.resolve(), SOURCE, out / 'repository')
    with zipfile.ZipFile(ROOT / 'docs/guide/downloads/ethernet-multicast-inputs.zip') as archive:
        expected = {p.relative_to(ROOT).as_posix(): p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()}
        assert len(expected) == 9 and set(archive.namelist()) == set(expected)
        for name, raw in expected.items(): assert archive.read(name) == raw
        archive.extractall(out / 'isolated')
    zipped, zip_provenance = measure(args.binary.resolve(), out / 'isolated/examples/guide/ethernet-multicast', out / 'zip')
    assert public == zipped
    evidence = {'status': 'passed', 'source_tag': 'v1.1.4', 'source_commit': COMMIT, 'cli_operations': 18,
                'zip_input_count': 9, 'single_variable': 'switches[0].multicast[0].egresses',
                'public_input_sha256': {p.relative_to(ROOT).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in SOURCE.rglob('*') if p.is_file()},
                'zip_reproduction': 'all simulation objects identical', 'conditions': public,
                'provenance': provenance, 'zip_provenance': zip_provenance}
    dest = ROOT / 'docs/verification/results/ethernet-multicast'; dest.mkdir(parents=True, exist_ok=True)
    raw = json.dumps(evidence, ensure_ascii=False, indent=2) + '\n'
    (dest / 'measurements.json').write_text(raw); (out / 'measurements.json').write_text(raw)
    print('PASS: 18 CLI operations; static multicast copies, parent paths, subscriptions, timings/CRC and isolated ZIP')


if __name__ == '__main__': main()
