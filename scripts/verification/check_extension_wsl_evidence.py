"""Verify saved public WSL evidence without running Cargo or changing originals."""
import hashlib
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PACKET = ROOT / 'docs/verification/results/extension-channel-cli-followup-2026-10-10/wsl-acceptance-600fd34'


def main():
    index = json.loads((PACKET / 'public-copy-index.json').read_text(encoding='utf-8'))
    assert len(index['all_saved_original_hashes']) == 49
    for row in index['copies']:
        p = (PACKET / row['path']).resolve()
        assert p.is_relative_to(PACKET.resolve())
        b = p.read_bytes()
        assert len(b) == row['public_bytes'] and hashlib.sha256(b).hexdigest() == row['public_sha256']
        assert all(s not in b for s in [b'/home/hideki/', b'/mnt/c/Users/'])
    execution = json.loads((PACKET / 'execution.json').read_text(encoding='utf-8'))
    spec = importlib.util.spec_from_file_location('runner', ROOT / 'scripts/verification/run_extension_acceptance.py')
    runner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runner)
    assert execution['status'] == 'passed_limited'
    assert [(x['kind'], x['target'], x['named_test']) for x in execution['commands']] == runner.TESTS
    for command in execution['commands']:
        assert command['exit_code'] == 0 and '--exact' in command['argv']
        assert runner.exact_pass((PACKET / command['streams']['stdout']['path']).read_text(encoding='utf-8'), command['named_test'])
    assert len(execution['required_checks']) == 3
    assert all(x['exit_code'] == 0 for x in execution['required_checks'])
    received = json.loads((PACKET / 'received-verification.json').read_text(encoding='utf-8'))
    assert received['source'] == '600fd34ba4d24d2dfb6d21a0e586176c798499fa'
    assert received['named_tests'] == 12 and received['log_streams_checked'] == 30
    assert received['workspace'] == {'groups': 32, 'passed': 610, 'failed': 0, 'ignored': 0}
    for row in execution['source_inputs']:
        b = (ROOT / row['path']).read_bytes()
        assert len(b) == row['bytes'] and hashlib.sha256(b).hexdigest() == row['sha256']
    print('PASS: saved WSL public copies, 12 exact regressions, required checks and unchanged source inputs')


if __name__ == '__main__':
    main()
