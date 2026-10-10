"""Actual trusted generation and mock collector tests; no GitHub writes."""
import base64
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from guide_merge_generated import verify_generated
from guide_scope_policy import GENERATED_REPORTS, Stop, allowed_path, safe_content
from guide_merge_runtime import blob_digest
from test_guide_merge_runtime import fixture, NUMBER

ROOT = Path(__file__).resolve().parents[1]
GUIDE = 'docs/guide/index.md'


class GeneratedTests(unittest.TestCase):
    def test_reports_never_pass_legacy_editable_policy(self):
        for path in GENERATED_REPORTS:
            with self.assertRaises(Stop): allowed_path(path)
            data = b'<script>untrusted</script>'
            with self.assertRaises(Stop): safe_content(path, base64.b64encode(data).decode(), blob_digest(data))

    def test_actual_generation_normal_revision_and_tampering(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            shutil.copytree(ROOT / 'docs', root / 'docs')
            # Execute only the real trusted generator, using a data snapshot.
            guide = (root / GUIDE).read_bytes().replace(b'1.1.', b'1.2.')
            (root / GUIDE).write_bytes(guide)
            result = subprocess.run([sys.executable, str(ROOT / 'scripts/generate_traceability.py'),
                                     '--root', str(root)], capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            files = {GUIDE: guide, **{p: (root / p).read_text(encoding='utf-8').encode('utf-8') for p in GENERATED_REPORTS}}
            self.assertTrue(verify_generated(ROOT, files))
            for path in GENERATED_REPORTS:
                bad = dict(files); bad[path] += b'<script>evil()</script>'
                with self.subTest(path=path), self.assertRaisesRegex(Stop, 'generated_report_mismatch'):
                    verify_generated(ROOT, bad)
            # A missing changed counterpart must not be repaired silently.
            bad = dict(files); bad.pop(next(iter(GENERATED_REPORTS)))
            with self.assertRaisesRegex(Stop, 'generated_report_mismatch'): verify_generated(ROOT, bad)
            with self.assertRaisesRegex(Stop, 'generated_without_guide_change'):
                verify_generated(ROOT, {p: files[p] for p in GENERATED_REPORTS})

    def test_generator_failure_and_wrong_checkout_stop(self):
        def fail(argv, **kwargs):
            self.assertEqual(argv[1], str(ROOT / 'scripts/generate_traceability.py'))
            self.assertNotIn('GH_TOKEN', kwargs['env'])
            self.assertNotIn('GITHUB_TOKEN', kwargs['env'])
            self.assertNotIn('PYTHONPATH', kwargs['env'])
            return type('Result', (), {'returncode': 1, 'stdout': b''})()
        with patch.dict('os.environ', {'GH_TOKEN':'mock-only','GITHUB_TOKEN':'mock-only','PYTHONPATH':'untrusted'}), self.assertRaisesRegex(Stop, 'trusted_generation_failed'):
            verify_generated(ROOT, {GUIDE: (ROOT / GUIDE).read_bytes()}, runner=fail)
        with self.assertRaisesRegex(Stop, 'generated_base_checkout_untrusted'):
            verify_generated(ROOT, {GUIDE: b'# Guide'}, trusted_sha='0'*40)

    def test_collector_derived_large_blob_and_rejects_unproven_or_renamed(self):
        path = sorted(GENERATED_REPORTS)[0]
        api, collector, env = fixture()
        data = b'x' * 2_400_000; sha = blob_digest(data)
        api.add('/git/blobs/'+sha, {'sha':sha,'encoding':'base64','size':len(data),
                                  'content':base64.b64encode(data).decode()})
        base = api.routes[('GET',api.prefix+'/git/trees/'+'d'*40+'?recursive=1')][0]['tree']
        head = api.routes[('GET',api.prefix+'/git/trees/'+'e'*40+'?recursive=1')][0]['tree']
        base.append({'path':path,'mode':'100644','type':'blob','sha':'f'*40})
        head.append({'path':path,'mode':'100644','type':'blob','sha':sha})
        rows = api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER)+'/files?per_page=100')][0]
        rows.append({'filename':path,'status':'modified','sha':sha})
        api.routes[('GET',api.prefix+'/pulls/'+str(NUMBER))][0]['changed_files']=2
        with patch('guide_merge_runtime.verify_generated', return_value=True) as proof:
            self.assertEqual(collector.collect(NUMBER)['files'][path], data)
            proof.assert_called_once()
            self.assertEqual(proof.call_args.kwargs['trusted_sha'], collector.trusted_sha)
        with patch('guide_merge_runtime.verify_generated', side_effect=Stop('generated_report_mismatch')):
            with self.assertRaises(Stop): collector.collect(NUMBER)
        rows[-1]['status']='renamed'; rows[-1]['previous_filename']=sorted(GENERATED_REPORTS)[1]
        with self.assertRaisesRegex(Stop, 'generated_report_must_be_modified'): collector.collect(NUMBER)
        self.assertEqual(api.writes, [])

    def test_bypass_unknown_is_never_empty(self):
        for value in [None, {}, '', False, [{'actor_id': 1}]]:
            api, collector, env = fixture()
            api.routes[('GET',api.prefix+'/rulesets/1?includes_parents=true')][0]['bypass_actors']=value
            with self.subTest(value=value), self.assertRaises(Stop): collector.collect(NUMBER)
            self.assertEqual(api.writes, [])


if __name__ == '__main__': unittest.main()
