"""Check the published deadline inputs and evidence without rerunning the CLI."""
import hashlib
import json
from pathlib import Path
import zipfile

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / 'docs/verification/results/ethernet-deadline-2026-10-10'
INPUTS = ROOT / 'examples/guide/ethernet-deadline'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    expected = {'short': '1155999', 'equal': '1156000', 'long': '1156001'}
    normalized = []
    for name, deadline in expected.items():
        workload = json.loads((INPUTS / f'{name}.json').read_text(encoding='utf-8'))
        assert len(workload['generators']) == 1
        g = workload['generators'][0]
        assert g['deadline_ps'] == deadline
        assert g['id'] == 'packet' and g['flow_id'] == 'flow_test'
        assert g['times_ps'] == ['0'] and g['priority'] == 0
        assert g['frame']['data'] == '' and g['frame']['dst_mac'] == '02:00:00:00:00:02'
        del g['deadline_ps']
        normalized.append(workload)
        text = (INPUTS / f'{name}.ini').read_text(encoding='utf-8')
        assert 'sim-time-limit = 3000000ps' in text
        assert 'model-profile = "ethernet.l2.qos.v1"' in text
        assert text.replace(f'workload = "{name}.json"', 'workload = "CASE.json"') == (INPUTS / 'short.ini').read_text(encoding='utf-8').replace('workload = "short.json"', 'workload = "CASE.json"')
    assert normalized[0] == normalized[1] == normalized[2]
    with zipfile.ZipFile(ROOT / 'docs/guide/downloads/ethernet-deadline-inputs.zip') as archive:
        files = {p.relative_to(ROOT).as_posix(): p.read_bytes() for p in INPUTS.rglob('*') if p.is_file()}
        assert len(files) == 8 and set(archive.namelist()) == set(files)
        assert all(archive.read(p) == data for p, data in files.items())
    original = (EVIDENCE / 'check_deadline_results.original.py').read_bytes()
    adjusted = (EVIDENCE / 'check_deadline_results.runtime-adjusted.py').read_bytes()
    assert digest(original) == 'ce7a5439e18d987e00d35eca63ad747c4f9dd401d0bdd480ea4b5f6fe2d1c018'
    assert original.count(b"meta['runtime_version'] == '1.1.4'") == 1
    assert adjusted == original.replace(b"meta['runtime_version'] == '1.1.4'", b"meta['runtime_version'] == '0.1.0'", 1)
    assert digest(adjusted) == '1d0193b06cc5ca40df20a11602e80f8fdf09edaa2492bba869e11a19f9d51592'
    for filename in ['deadline-analysis.json', 'zip-deadline-analysis.json']:
        data = json.loads((EVIDENCE / filename).read_text(encoding='utf-8'))['evidence']
        assert data['measured_source'] == 'b45515644dfc65c205216d564d5bf7ef00280622'
        for case in data['conditions']:
            records = case['all_model_records']
            assert len(records) == 5
            assert len(case['all_metric_records']) == 155 and len(case['all_summary_metrics']) == 373
            frames = [r for r in records if r['schema_name'] == 'ethernet.frame']
            assert len(frames) == 1 and frames[0]['data']['deadline_ps'] == expected[case['condition']]
            b = [r for r in records if r['schema_name'] == 'ethernet.reception' and r['subject'] == 'Main.b']
            assert len(b) == 1 and b[0]['data']['status'] == 'received' and b[0]['data']['ready_ps'] == '1156000'
    for entry in json.loads((EVIDENCE / 'input-snapshots.json').read_text(encoding='utf-8')):
        assert len(entry['sources']) == 4
        for s in entry['sources']:
            assert digest(s['content_utf8'].encode()) == s['sha256']
    review = json.loads((EVIDENCE / 'visual-review.json').read_text(encoding='utf-8'))
    for entry in review['images']:
        filename = 'ethernet-deadline-' + Path(entry['path']).name
        assert digest((ROOT / 'docs/guide/assets' / filename).read_bytes()) == entry['sha256']
    for p in EVIDENCE.glob('*.json'):
        s = p.read_text(encoding='utf-8')
        assert all(x not in s for x in ['C:\\\\Users\\\\', '/mnt/c/Users/', '/home/hideki/']), p.name
    print('PASS: eight one-variable inputs/ZIP, exact checker copy, six complete record/metric sets, input snapshots and four original PNG hashes')


if __name__ == '__main__':
    main()
