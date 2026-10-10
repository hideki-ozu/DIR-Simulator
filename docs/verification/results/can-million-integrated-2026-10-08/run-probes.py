#!/usr/bin/env python3
"""Complete current million-request conditions, with formal follow-up for fast probes."""
import datetime
import hashlib
import json
from pathlib import Path
import shutil
import sys

ROOT = Path('/tmp/dir-million-integrated-2026-10-08')
REPO = Path('/home/hideki/DIR-Simulator')
sys.path.insert(0, str(ROOT / 'helpers'))
from measure_can_million import attempt, evaluate, save, utc, GIB


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def pins(before):
    expected = before['source_sha256']
    paths = [*REPO.joinpath('crates').rglob('*'), *(REPO / n for n in (
        'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml'))]
    actual = {str(p.relative_to(REPO)): sha(p) for p in paths if p.is_file() and not p.is_symlink()}
    assert actual == expected, 'Current product source changed'
    assert all(sha(ROOT / 'source' / p) == h for p, h in expected.items())
    assert sha(Path(before['binary']['path'])) == before['binary']['sha256']
    assert all(sha(ROOT / 'inputs' / p) == h for p, h in before['input_sha256'].items())
    assert all(sha(ROOT / p) == h for p, h in before['helper_sha256'].items())
    assert all(sha(REPO / p) == h for p, h in before['build_documents_sha256'].items())
    return {'verified_at_utc': utc(), 'source_file_count': len(actual), 'all_pins_match': True}


def run_one(before, condition, folder, kind, repeat):
    condition['pre_attempt_pins'] = pins(before)
    result = attempt(Path(before['binary']['path']), ROOT / 'inputs' / condition['ini'],
                     folder, 26 * GIB, wall_limit_seconds=1200)
    result.update(kind=kind, repeat=repeat, condition=condition['rho'])
    output = folder / 'output'
    if result['completed']:
        generated = [x for x in result['selected_summary']
                     if x['target'] == '$all' and x['metric'] == 'generated']
        if len(generated) != 1 or generated[0]['value'] != '1000000':
            result['completed'] = False
            result['verification_error'] = 'Expected 1000000 published generated requests'
        shutil.copyfile(output / 'manifest.json', folder / 'manifest.json')
        with (output / 'results.json').open('rb') as stream:
            prefix = stream.read(1024 * 1024)
        boundary = prefix.index(b',"schema_version":1,"simulation":')
        metadata = json.loads(prefix[:boundary] + b'}')['metadata']
        save(folder / 'metadata.json', metadata)
        result['completion_wall_target_met'] = result['measurements']['wall_seconds'] <= 120
        result['full_process_rss_target_met'] = result['max_rss_bytes'] <= 2 * GIB
    else:
        result['completion_wall_target_met'] = False if result['measurements']['wall_seconds'] > 120 else None
        result['full_process_rss_target_met'] = False if result['max_rss_bytes'] > 2 * GIB else None
    result['post_attempt_pins'] = pins(before)
    save(folder / 'attempt-record.json', result)
    if output.exists():
        shutil.rmtree(output)
    result['large_outputs_removed_after_recording'] = True
    save(folder / 'attempt-record.json', result)
    return result


def main():
    before = json.loads((ROOT / 'before.json').read_text())
    measurement_root = ROOT / 'measurements'
    measurement_root.mkdir(exist_ok=False)
    record = {
        'schema_version': 1, 'status': 'running', 'started_at_utc': utc(),
        'qualification': 'WSL2 integrated completion probes. Each condition completes once; formal warmup plus three repeats follows only when the initial probe meets the time target. This is not a native-baseline qualification.',
        'before_sha256': sha(ROOT / 'before.json'), 'binary': before['binary'],
        'targets': before['targets'], 'source_sha256': before['source_sha256'],
        'wall_observation_limit_seconds': 1200, 'conditions': [],
    }
    path = ROOT / 'measurement.json'
    assert not path.exists()
    save(path, record)
    try:
        for rho in ('0.30', '0.90', '1.20'):
            condition = {'rho': rho, 'ini': f'rho-{rho}.ini', 'status': 'running', 'attempts': []}
            record['conditions'].append(condition)
            save(path, record)
            result = run_one(before, condition, measurement_root / f'rho-{rho}-probe', 'completion_probe', 0)
            condition['attempts'].append(result)
            save(path, record)
            print(json.dumps({'completed_probe': rho, 'completed': result['completed'],
                              'wall_seconds': result['measurements']['wall_seconds'],
                              'max_rss_bytes': result['max_rss_bytes'],
                              'wall_target_met': result['completion_wall_target_met'],
                              'memory_target_met': result['full_process_rss_target_met']}), flush=True)
            if result['completed'] and result['completion_wall_target_met']:
                result['kind'] = 'warmup'
                for repeat in (1, 2, 3):
                    measured = run_one(before, condition, measurement_root / f'rho-{rho}-{repeat}',
                                       'measurement', repeat)
                    condition['attempts'].append(measured)
                    condition['formal_evaluation'] = evaluate(condition['attempts'])
                    save(path, record)
            else:
                condition['formal_evaluation'] = None
                condition['formal_repeats_not_run_reason'] = 'Initial completion probe exceeded the time target or was incomplete; no formal median or repeat determinism claim.'
            condition['status'] = 'complete'
            save(path, record)
        record['status'] = 'complete'
        record['finished_at_utc'] = utc()
        save(path, record)
        print(json.dumps({'status': 'complete', 'record': str(path)}), flush=True)
    except BaseException as error:
        record['status'] = 'failed_or_interrupted'
        record['failure'] = repr(error)
        record['finished_at_utc'] = utc()
        save(path, record)
        raise


if __name__ == '__main__':
    main()
