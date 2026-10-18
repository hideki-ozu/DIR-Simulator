"""Independently check Metric V2 event invariance and integer-area windows."""
import argparse
import csv
from fractions import Fraction
import hashlib
import json
import math
from pathlib import Path
import subprocess
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / 'examples/guide/metric-windows'
COMMIT = '397900451015c9bca6a33d24feac0a88b9b7b8fe'
WINDOWS = (4, 7, 10)
def load(p): return json.loads(p.read_text())
def digest(p): return hashlib.sha256(p.read_bytes()).hexdigest()

def check_inputs():
    normalized = [(SOURCE / f'window-{w}.ini').read_text().replace(f'metrics-window = {w}ps', 'metrics-window = WINDOW') for w in WINDOWS]
    assert normalized[0] == normalized[1] == normalized[2]
    assert (SOURCE / 'ned/demo/model.ned').read_bytes() == (ROOT / 'examples/control-checkpoint/ned/demo/model.ned').read_bytes()
    assert (SOURCE / 'reference/control_checkpoint.rs').read_bytes() == (ROOT / 'crates/dir-simulator/examples/control_checkpoint.rs').read_bytes()

def expected_area(a, b):
    # Independent piecewise input contract: [0,3)=2, [3,7)=4, [7,10)=1.
    return sum(max(0, min(b, end) - max(a, start)) * value for start, end, value in [(0, 3, 2), (3, 7, 4), (7, 10, 1)])

def measure(binary, example, source, output):
    output.mkdir(parents=True, exist_ok=False); values = {}; commands = []
    for w in WINDOWS:
        name = f'window-{w}'; result = output / name
        for executable, args in [(example, ['run', str(source / (name + '.ini')), str(result)]),
                                 (binary, ['view', '--input', str(result / 'completed/results.json'), '--output', str(output / (name + '-viewer.html'))])]:
            p = subprocess.run([str(executable), *args], capture_output=True, text=True)
            commands.append({'command': [str(executable), *args], 'exit_code': p.returncode, 'stdout': p.stdout, 'stderr': p.stderr})
            assert p.returncode == 0, p.stderr
        folder = result / 'completed'; manifest = load(folder / 'manifest.json')
        assert manifest['status'] == 'complete' and manifest['partial'] is False
        for f in manifest['files']:
            path = folder / f['name']; assert str(path.stat().st_size) == f['bytes'] and digest(path) == f['sha256']
        assert (folder / 'diagnostics.jsonl').read_text() == ''
        d = load(folder / 'results.json'); meta, sim = d['metadata'], d['simulation']
        assert meta['git_commit'] == COMMIT and meta['git_dirty'] == 'false' and meta['binary_sha256'] == digest(example)
        assert d['control']['state'] == 'Completed' and sim['end_ps'] == '10' and sim['committed_events'] == '3' and sim['pending_events'] == '0' and not sim['partial']
        assert [(r['time_ps'], r['data']['count']) for r in sim['model_records']] == [('3', '1'), ('7', '2'), ('7', '3')]
        assert [(r['metric'], r['time_ps'], r['value']) for r in sim['records']] == [('demo.depth', '3', '4'), ('demo.minimum', '3', '5'), ('demo.depth', '7', '9'), ('demo.depth', '7', '1'), ('demo.minimum', '7', '1')]
        global_rows = [r for r in sim['summary'] if r['start_ps'] is None]
        assert [(r['metric'], r['value'], r['sample_count']) for r in global_rows] == [('demo.depth', 2.5, '3'), ('demo.minimum', '1', '2')]
        expected_windows = [(a, min(a+w, 10)) for a in range(0, 10, w)]
        gauge = [r for r in sim['summary'] if r['metric'] == 'demo.depth' and r['start_ps'] is not None]
        point = [r for r in sim['summary'] if r['metric'] == 'demo.minimum' and r['start_ps'] is not None]
        assert [(int(r['start_ps']), int(r['end_ps'])) for r in gauge] == expected_windows
        areas = []
        for row, minimum, (a, b) in zip(gauge, point, expected_windows):
            area = expected_area(a, b); areas.append(area)
            assert math.isclose(row['value'], float(Fraction(area, b-a)), rel_tol=0, abs_tol=1e-15)
            assert row['sample_count'] == str(sum(a <= t < b for t in [3, 7, 7]))
            samples = [v for t, v in [(3, 5), (7, 1)] if a <= t < b]
            assert minimum['sample_count'] == str(len(samples)) and minimum['value'] == (str(min(samples)) if samples else None)
        assert sum(areas) == 25
        csv_rows = list(csv.DictReader((folder / 'summary.csv').open()))
        assert len(csv_rows) == len(sim['summary'])
        for saved, row in zip(csv_rows, sim['summary']):
            for key in ('metric', 'sample_count', 'start_ps', 'end_ps'):
                assert saved[key] == ('' if row[key] is None else str(row[key]))
            if row['value'] is None: assert saved['value'] == ''
            else: assert float(saved['value']) == float(row['value'])
        values[name] = {'simulation': sim, 'metadata': {k: meta[k] for k in ('git_commit', 'git_dirty', 'binary_sha256', 'build_source_sha256', 'sources')},
                        'independent_areas_item_ps': areas, 'window_average_fractions': [str(Fraction(area, b-a)) for area, (a, b) in zip(areas, expected_windows)],
                        'viewer_html_sha256': digest(output / (name + '-viewer.html'))}
    first = values['window-4']['simulation']
    for v in values.values():
        for key in ('records', 'model_records', 'committed_events', 'last_event_time_ps', 'end_ps', 'termination'):
            assert v['simulation'][key] == first[key]
    (output / 'commands.json').write_text(json.dumps(commands, ensure_ascii=False, indent=2) + '\n')
    return values

def main():
    p = argparse.ArgumentParser(description=__doc__); p.add_argument('--binary', type=Path, required=True); p.add_argument('--example', type=Path, required=True); p.add_argument('--output', type=Path, required=True)
    p.add_argument('--publish-report', action='store_true', help='also replace the tracked measurement report after a successful run')
    a = p.parse_args(); check_inputs(); out = a.output.resolve(); out.mkdir(parents=True, exist_ok=False)
    repo = measure(a.binary.resolve(), a.example.resolve(), SOURCE, out / 'repository')
    expected = {p.relative_to(ROOT).as_posix(): p.read_bytes() for p in SOURCE.rglob('*') if p.is_file()}
    with zipfile.ZipFile(ROOT / 'docs/guide/downloads/metric-windows-inputs.zip') as z:
        assert set(z.namelist()) == set(expected)
        for name, contents in expected.items(): assert z.read(name) == contents
        z.extractall(out / 'isolated')
    zipped = measure(a.binary.resolve(), a.example.resolve(), out / 'isolated/examples/guide/metric-windows', out / 'zip')
    assert {k: v['simulation'] for k, v in repo.items()} == {k: v['simulation'] for k, v in zipped.items()}
    report = {'generated_by': 'scripts/verify_guide_metric_windows.py', 'status': 'passed', 'source_tag': 'v1.1.6', 'source_commit': COMMIT,
              'single_variable': 'metrics-window', 'cli_operations': 12, 'zip_input_count': len(expected),
              'input_sha256': {p.relative_to(ROOT).as_posix(): digest(p) for p in SOURCE.rglob('*') if p.is_file()}, 'conditions': repo, 'zip_conditions': zipped}
    raw = json.dumps(report, ensure_ascii=False, indent=2) + '\n'
    (out / 'measurements.json').write_text(raw)
    if a.publish_report:
        dest = ROOT / 'docs/verification/results/metric-windows'; dest.mkdir(parents=True, exist_ok=True)
        (dest / 'measurements.json').write_text(raw)
    print('PASS: identical successful events/records, global25/10, windows4/7/10 independent areas, point half-open boundaries/null, CSV and isolated ZIP equality')

if __name__ == '__main__': main()
