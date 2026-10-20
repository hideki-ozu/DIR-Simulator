"""Exercise #121 with actual formal CLI runs and Chromium captures.

NODE_PATH must resolve Playwright; --output must be a fresh directory. The saved
acceptance.json is a compact execution record, not an alias for temporary raw files.
"""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SLUGS = ('ethernet-beb-seed', 'axi-outstanding', 'ethernet-cbs-gate', 'gateway-rx-capacity')
EXPECTED_CAPTURES = dict(zip(SLUGS, (4, 4, 8, 4)))


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def snapshot(paths):
    return {str(p): {'bytes': p.stat().st_size, 'sha256': sha(p)} for p in sorted(set(paths)) if p.is_file()}


def protected_paths():
    files = []
    for slug in SLUGS:
        files.extend((ROOT / 'docs/verification/results' / slug).rglob('*'))
        files.extend((ROOT / 'examples/guide' / slug).rglob('*'))
        files.extend((ROOT / 'docs/guide/assets').glob(f'guide-{slug}-*.png'))
        files.append(ROOT / 'docs/guide/downloads' / f'{slug}-inputs.zip')
    return files


def check_capture(output, selected):
    summary = json.loads((output / 'capture-summary.json').read_text())
    assert summary['status'] == 'passed' and summary['selected_experiments'] == list(selected)
    assert {r['experiment'] for r in summary['reports']} == set(selected)
    checked = []
    for entry in summary['reports']:
        report_path = output / entry['path']
        assert sha(report_path) == entry['sha256']
        report = json.loads(report_path.read_text())
        base = report_path.parent
        assert report['status'] == 'passed'
        measured_path = (base / report['measurements']['path']).resolve()
        assert sha(measured_path) == report['measurements']['sha256']
        measured = json.loads(measured_path.read_text())
        assert report['input_sha256'] == measured['input_sha256']
        assert report['binary_sha256'] == measured['binary_sha256']
        assert report['source_commit'] == measured['source_commit']
        assert len(report['captures']) == entry['capture_count'] == EXPECTED_CAPTURES[entry['experiment']]
        for condition, data in report['conditions'].items():
            assert data['rollback_equal']
            assert data['errors'] == data['external_requests'] == data['failed_requests'] == []
            assert condition.rsplit('-', 1)[0] in measured['conditions']
        for cap in report['captures']:
            assert cap['condition'] in measured['conditions']
            assert cap['viewport'] in ({'name': 'desktop', 'width': 1440, 'height': 1000}, {'name': 'mobile', 'width': 390, 'height': 844})
            for kind in ('viewer', 'results', 'manifest'):
                assert sha((base / cap[kind + '_path']).resolve()) == cap[kind + '_sha256']
            image = (base / cap['screenshot']).resolve()
            image.relative_to(output.resolve())
            raw = image.read_bytes()
            assert raw[:8] == b'\x89PNG\r\n\x1a\n' and sha(image) == cap['sha256']
            assert int.from_bytes(raw[16:20], 'big') == cap['width'] <= cap['viewport']['width']
            assert int.from_bytes(raw[20:24], 'big') == cap['height'] > 0
        checked.append({'experiment': entry['experiment'], 'report_sha256': entry['sha256'], 'captures': len(report['captures']), 'actual_paths_hashes_and_viewports_verified': True})
    return checked


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--source-root', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    output = a.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    before = snapshot(protected_paths())
    commands = []

    def execute(args, success=True):
        proc = subprocess.run(args, cwd=ROOT, text=True, capture_output=True)
        commands.append({'command': list(map(str, args)), 'exit_code': proc.returncode, 'stdout': proc.stdout, 'stderr': proc.stderr})
        assert (proc.returncode == 0) == success, proc.stderr
        return proc

    capture_script = str(ROOT / 'scripts/verify_capacity_comparisons_viewer.cjs')
    checks = []
    for label, selected in [('all', SLUGS), ('axi', ('axi-outstanding',)), ('gateway', ('gateway-rx-capacity',))]:
        evidence = output / f'{label}-measurements'
        measure = ['python3', str(ROOT / 'scripts/verify_guide_capacity_comparisons.py'), '--binary', str(a.binary.resolve()), '--source-root', str(a.source_root.resolve()), '--output', str(evidence)]
        selection = [] if len(selected) == 4 else ['--only', selected[0]]
        execute(measure + selection)
        assert {p.name for p in evidence.iterdir() if p.is_dir()} == set(selected), 'single experiment must not create unrelated inputs'
        raw_before = snapshot(evidence.rglob('*'))
        captures = evidence / 'viewer-captures' if label == 'all' else output / f'{label}-captures'
        capture = ['node', capture_script, str(evidence), *selection]
        if label != 'all':
            capture.extend(['--output', str(captures)])
        execute(capture)
        assert snapshot(Path(name) for name in raw_before) == raw_before, 'capture changed input measurement artifacts'
        checked = check_capture(captures, selected)
        saved = snapshot(captures.rglob('*'))
        rejected = execute(capture, success=False)
        assert 'refuse existing capture output' in rejected.stderr
        assert snapshot(captures.rglob('*')) == saved, 'rejected rerun modified existing capture output'
        checks.append({'selection': list(selected), 'report_checks': checked, 'same_output_refused_without_mutation': True, 'measurement_inputs_unchanged': True})
        if label == 'axi':
            second = output / 'axi-captures-2'
            execute(['node', capture_script, str(evidence), *selection, '--output', str(second)])
            check_capture(second, selected)
            assert snapshot(captures.rglob('*')) == saved
            checks[-1]['fresh_other_output_succeeded'] = True
    invalid = output / 'invalid-selection-output'
    rejected = execute(['node', capture_script, str(output / 'axi-measurements'), '--only', 'missing', '--output', str(invalid)], success=False)
    assert 'unknown --only experiment' in rejected.stderr and not invalid.exists()
    after = snapshot(protected_paths())
    assert before == after, 'public evidence, images, inputs or ledgers changed'
    report = {'status': 'passed', 'issue': 121, 'generated_by': 'scripts/verify_issue121_capture_workflow.py',
              'commands': commands, 'checks': checks, 'invalid_selection_rejected_before_output_creation': True,
              'protected_artifact_count': len(before), 'protected_artifacts_unchanged': True,
              'protected_artifacts': {str(Path(k).relative_to(ROOT)): v for k, v in before.items()}}
    (output / 'acceptance.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print('PASS: fresh full/AXI/Gateway capture, same-output refusal, fresh-other output, actual report/PNG/input hashes, original artifacts preserved')


if __name__ == '__main__':
    main()
