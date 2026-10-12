"""Regression checks for bundled third-party notice verification.

Run after building the MkDocs guide with:

    python3 tests/test_third_party_notices.py

Each case gives the checker isolated copies of its inputs so negative tests
cannot alter the working tree.
"""

import contextlib
import importlib.util
import io
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHECKER_PATH = ROOT / 'scripts/check_third_party_notices.py'
EVIDENCE_SOURCE = ROOT / 'docs/verification/results/release-native-distribution-audit-2026-10-08'
GUIDE_SOURCE = ROOT / 'build/guide'
SUPPLEMENT_RELATIVE = Path('docs/verification/results/signal-hook-notice-supplement-2026-10-12')
SUPPLEMENT_SOURCE = ROOT / SUPPLEMENT_RELATIVE


def load_checker():
    spec = importlib.util.spec_from_file_location('third_party_notice_checker', CHECKER_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f'Cannot load checker at {CHECKER_PATH}')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ThirdPartyNoticeCheckerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not GUIDE_SOURCE.is_dir():
            raise RuntimeError('Build the MkDocs guide before running these tests')
        cls.checker = load_checker()
        cls.manifest = json.loads((EVIDENCE_SOURCE / 'license-notice-manifest.json').read_text())
        cls.supplement = json.loads((SUPPLEMENT_SOURCE / 'supplement.json').read_text())

    def _cargo_entry(self, name, version):
        return next(
            row for row in self.manifest['cargo']
            if row['name'] == name and row['version'] == version
        )

    def _supplement_entry(self, name):
        return next(row for row in self.supplement['packages'] if row['name'] == name)

    def _guide_entry(self, path_fragment):
        return next(row for row in self.manifest['guide'] if path_fragment in row['path'])

    def _copy_inputs(self, case_dir):
        root = case_dir / 'root'
        evidence = case_dir / 'evidence'
        guide = case_dir / 'build-guide'

        (root / 'docs').mkdir(parents=True)
        shutil.copy2(ROOT / 'Cargo.lock', root / 'Cargo.lock')
        shutil.copytree(ROOT / 'docs/third-party', root / 'docs/third-party')
        shutil.copytree(ROOT / 'docs/guide/assets/licenses', root / 'docs/guide/assets/licenses')
        shutil.copytree(SUPPLEMENT_SOURCE, root / SUPPLEMENT_RELATIVE)

        evidence.mkdir()
        shutil.copy2(EVIDENCE_SOURCE / 'license-notice-manifest.json', evidence)
        shutil.copy2(EVIDENCE_SOURCE / self.manifest['audit_zip'], evidence)
        shutil.copytree(GUIDE_SOURCE, guide)
        return root, evidence, guide

    @contextlib.contextmanager
    def _isolated_inputs(self, mutate=None):
        with tempfile.TemporaryDirectory(prefix='third-party-notice-test-') as temporary:
            case_dir = Path(temporary)
            root, evidence, guide = self._copy_inputs(case_dir)
            if mutate is not None:
                mutate(root, guide)
            yield root, evidence, guide, case_dir

    def _invoke_checker(self, root, evidence, guide, output):
        old_root = self.checker.ROOT
        old_evidence = self.checker.EVIDENCE
        old_argv = sys.argv
        try:
            self.checker.ROOT = root
            self.checker.EVIDENCE = evidence
            sys.argv = [
                str(CHECKER_PATH), '--guide-dir', str(guide), '--output', str(output)
            ]
            with contextlib.redirect_stdout(io.StringIO()):
                self.checker.main()
        finally:
            self.checker.ROOT = old_root
            self.checker.EVIDENCE = old_evidence
            sys.argv = old_argv
        return json.loads(output.read_text())

    def _assert_checker_fails(self, mutate, exception_type=AssertionError):
        with self._isolated_inputs(mutate) as (root, evidence, guide, case_dir):
            with self.assertRaises(exception_type):
                self._invoke_checker(root, evidence, guide, case_dir / 'checker-report.json')

    def _replace_once(self, path, old, new):
        data = path.read_bytes()
        if data.count(old) != 1:
            raise AssertionError(f'Expected one occurrence of {old!r} in {path}')
        path.write_bytes(data.replace(old, new, 1))

    def test_clean_copied_inputs_pass(self):
        with self._isolated_inputs() as (root, evidence, guide, case_dir):
            report = self._invoke_checker(
                root, evidence, guide, case_dir / 'checker-report.json'
            )
        self.assertEqual(report['status'], 'passed_originals_and_notice_correspondence')
        self.assertEqual(report['locked_packages_checked'], 44)
        self.assertEqual(report['historical_locked_packages_checked'], 42)
        self.assertEqual(report['supplemental_locked_packages_checked'], 2)
        self.assertEqual(report['supplemental_manifest_sha256'], self.checker.SUPPLEMENT_SHA256)
        self.assertEqual(report['crate_license_originals_checked'], 93)
        self.assertEqual(report['source_notice_bundles_checked'], 10)
        self.assertEqual(report['guide_license_originals_checked'], 8)
        self.assertGreater(len(report['fresh_guide_assets_compared']), 0)

    def test_truncated_original_fails(self):
        entry = self._cargo_entry('ascii', '1.1.0')['license_files'][0]

        def mutate(root, _guide):
            path = root / entry['path']
            path.write_bytes(path.read_bytes()[:-1])

        self._assert_checker_fails(mutate)

    def test_missing_unicode_original_fails(self):
        entry = self._cargo_entry('unicode-ident', '1.0.26')
        license_file = next(
            row for row in entry['license_files']
            if row['upstream_path'] == 'LICENSE-UNICODE'
        )

        def mutate(root, _guide):
            (root / license_file['path']).unlink()

        self._assert_checker_fails(mutate, FileNotFoundError)

    def test_changed_lock_version_fails(self):
        old = b'name = "ascii"\nversion = "1.1.0"'
        new = b'name = "ascii"\nversion = "1.1.1"'

        def mutate(root, _guide):
            self._replace_once(root / 'Cargo.lock', old, new)

        self._assert_checker_fails(mutate)

    def test_changed_lock_checksum_fails(self):
        checksum = self._cargo_entry('ascii', '1.1.0')['checksum'].encode()
        replacement = checksum[:-1] + (b'0' if checksum[-1:] != b'0' else b'1')

        def mutate(root, _guide):
            self._replace_once(root / 'Cargo.lock', checksum, replacement)

        self._assert_checker_fails(mutate)

    def test_dropped_ledger_row_fails(self):
        prefix = '| `ascii` | `1.1.0` |'

        def mutate(root, _guide):
            path = root / 'docs/third-party/採用物台帳.md'
            lines = path.read_text().splitlines(keepends=True)
            kept = [line for line in lines if not line.startswith(prefix)]
            if len(kept) == len(lines):
                raise AssertionError('Could not find the ascii ledger row to remove')
            path.write_text(''.join(kept))

        self._assert_checker_fails(mutate)

    def test_tampered_source_notice_fails(self):
        source_notice = self._cargo_entry('rustix', '0.38.44')['source_notice_path']

        def mutate(root, _guide):
            self._replace_once(
                root / source_notice,
                b'Source notice excerpts',
                b'Altered notice excerpts',
            )

        self._assert_checker_fails(mutate)

    def test_tampered_popper_repository_notice_fails(self):
        entry = self._guide_entry('popper-2.11.8-MIT.txt')

        def mutate(root, _guide):
            self._replace_once(
                root / entry['path'], b'Federico Zivolo', b'Altered Holder'
            )

        self._assert_checker_fails(mutate)

    def test_tampered_popper_built_asset_fails(self):
        entry = self._guide_entry('popper-2.11.8-MIT.txt')
        relative_path = entry['path'].removeprefix('docs/guide/')

        def mutate(_root, guide):
            self._replace_once(
                guide / relative_path, b'Federico Zivolo', b'Altered Holder'
            )

        self._assert_checker_fails(mutate)

    def test_missing_new_notice_fails(self):
        entry = self._supplement_entry('signal-hook')['license_files'][0]
        self._assert_checker_fails(lambda root, _guide: (root / entry['path']).unlink(), FileNotFoundError)

    def test_tampered_new_notice_fails(self):
        entry = self._supplement_entry('signal-hook-registry')['license_files'][1]
        def mutate(root, _guide):
            path = root / entry['path']
            path.write_bytes(path.read_bytes() + b'altered')
        self._assert_checker_fails(mutate)

    def test_missing_supplement_fails(self):
        self._assert_checker_fails(
            lambda root, _guide: (root / SUPPLEMENT_RELATIVE / 'supplement.json').unlink(),
            FileNotFoundError)

    def test_changed_supplement_manifest_fails(self):
        def mutate(root, _guide):
            path = root / SUPPLEMENT_RELATIVE / 'supplement.json'
            data = json.loads(path.read_text())
            data['packages'][0]['license_choice'] = 'MIT'
            path.write_text(json.dumps(data))
        self._assert_checker_fails(mutate)

    def test_rehashed_tampered_new_notice_and_manifest_still_fail(self):
        def mutate(root, _guide):
            path = root / SUPPLEMENT_RELATIVE / 'supplement.json'
            data = json.loads(path.read_text())
            entry = data['packages'][0]['license_files'][0]
            notice = root / entry['path']
            altered = notice.read_bytes()[:-1]
            notice.write_bytes(altered)
            entry['sha256'] = self.checker.sha(altered)
            entry['bytes'] = len(altered)
            path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n')
        self._assert_checker_fails(mutate)

    def test_changed_supplement_original_metadata_fails(self):
        entry = self._supplement_entry('signal-hook')['metadata_files'][0]
        def mutate(root, _guide):
            path = root / entry['path']
            path.write_bytes(path.read_bytes() + b'\n# changed\n')
        self._assert_checker_fails(mutate)

    def test_changed_new_lock_checksum_fails(self):
        checksum = self._supplement_entry('signal-hook')['checksum'].encode()
        replacement = checksum[:-1] + (b'0' if checksum[-1:] != b'0' else b'1')
        self._assert_checker_fails(lambda root, _guide: self._replace_once(root / 'Cargo.lock', checksum, replacement))

    def test_changed_new_lock_version_fails(self):
        self._assert_checker_fails(lambda root, _guide: self._replace_once(
            root / 'Cargo.lock', b'name = "signal-hook-registry"\nversion = "1.4.8"',
            b'name = "signal-hook-registry"\nversion = "1.4.9"'))

    def test_dropped_new_ledger_row_fails(self):
        def mutate(root, _guide):
            path = root / 'docs/third-party/採用物台帳.md'
            path.write_text(''.join(line for line in path.read_text().splitlines(keepends=True)
                                    if not line.startswith('| `signal-hook` | `0.3.18` |')))
        self._assert_checker_fails(mutate)

    def test_missing_new_readme_license_declaration_fails(self):
        entry = self._supplement_entry('signal-hook')['license_declaration_excerpt']
        self._assert_checker_fails(lambda root, _guide: (root / entry['path']).unlink(), FileNotFoundError)


if __name__ == '__main__':
    unittest.main(verbosity=2)
