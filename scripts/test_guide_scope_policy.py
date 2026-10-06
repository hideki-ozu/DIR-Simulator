"""Synthetic safety tests for the disabled policy proposal; no network calls."""
import base64
import copy
import hashlib
import unittest
from guide_scope_policy import evaluate, GUIDES, IMAGES, SAMPLES, REQUIRED_CHECKS, WORKFLOW_PATH, REPOSITORY


def file_row(path='docs/guide/CANの調停.md', content=b'# CAN\n'):
    sha = hashlib.sha1(b'blob ' + str(len(content)).encode() + b'\0' + content).hexdigest()
    return {'path': path, 'status': 'modified', 'base_type': 'blob', 'head_type': 'blob',
            'base_mode': '100644', 'head_mode': '100644', 'content_sha': sha, 'head_blob_sha': sha,
            'content_base64': base64.b64encode(content).decode()}


def fixture():
    head, base = 'a' * 40, 'b' * 40
    return {'schema_version': 1,
            'pr': {'number': 100, 'repository': REPOSITORY, 'head_repository': REPOSITORY,
                   'author': 'hideki-ozu', 'base_ref': 'main', 'head_sha': head, 'base_sha': base,
                   'draft': False, 'state': 'open', 'mergeable': True, 'merge_state': 'clean', 'changed_files': 1},
            'fresh_pr': {'head_sha': head, 'base_sha': base, 'draft': False, 'state': 'open'},
            'protection': {'enforced': True, 'strict': True, 'no_bypass': True,
                           'required_checks': sorted(REQUIRED_CHECKS), 'source': 'github-actions', 'read_complete': True},
            'file_listing': {'all_pages_read': True, 'has_next_page': False, 'truncated': False,
                             'head_sha': head, 'base_sha': base, 'files': [file_row()]},
            'trees': {'head_sha': head, 'base_sha': base, 'complete': True, 'truncated': False},
            'trusted_main_sha': base,
            'evidence': {'all_pages_read': True, 'has_next_page': False, 'head_sha': head, 'base_sha': base,
                         'trusted_workflow_sha': base, 'workflow_path': WORKFLOW_PATH,
                         'repository': REPOSITORY, 'event': 'pull_request', 'pull_number': 100,
                         'run_id': 123, 'run_attempt': 2,
                         'checks': [{'name': name, 'status': 'completed', 'conclusion': 'success',
                                     'run_id': 123, 'run_attempt': 2} for name in sorted(REQUIRED_CHECKS)],
                         'samples_reproduced': True, 'public_asset_scan': True, 'images_decoded_reencoded': True,
                         'no_pr_code_in_privileged_job': True, 'workflow_matches_trusted_main': True}}


class PolicyTests(unittest.TestCase):
    def rejected(self, s):
        result = evaluate(s)
        self.assertFalse(result['review_candidate'], result)
        self.assertFalse(result['merge_authorized'])

    def test_positive_is_still_disabled(self):
        result = evaluate(fixture())
        self.assertTrue(result['review_candidate'])
        self.assertFalse(result['merge_authorized'])

    def test_exact_allowlist(self):
        for path in GUIDES | IMAGES | SAMPLES:
            with self.subTest(path=path):
                content = b'\x89PNG\r\n\x1a\nsynthetic' if path in IMAGES else (
                    b'{"schema_version":1,"generators":[]}' if path.endswith('.json') else b'# text\n')
                s = fixture(); s['file_listing']['files'] = [file_row(path, content)]
                self.assertTrue(evaluate(s)['review_candidate'])
                self.assertFalse(evaluate(s)['merge_authorized'])

    def test_forbidden_paths(self):
        paths = ['docs/Qiita移植メモ.md', 'docs/guide/Qiita移植メモ.md', 'docs/guide/new.md',
                 'docs/guide/assets/test.svg', 'docs/guide/downloads/can-guide-inputs.zip',
                 'docs/guide/assets/guide.js', 'docs/guide/raw.html', 'crates/dir-simulator/src/main.rs',
                 'scripts/verify_guide.py', '.github/workflows/guide-pages.yml', 'mkdocs.yml',
                 'requirements-guide.txt', 'README.md', 'examples/guide/can/models/demo/Main.ned',
                 'docs/guide/../guide/index.md', '/docs/guide/index.md', 'docs\\guide\\index.md']
        for path in paths:
            with self.subTest(path=path):
                s = fixture(); s['file_listing']['files'][0]['path'] = path; self.rejected(s)

    def test_unverified_pr_or_policy(self):
        cases = [('pr', 'number', 26), ('pr', 'draft', True), ('pr', 'state', 'closed'),
                 ('pr', 'mergeable', None), ('pr', 'merge_state', 'dirty'), ('pr', 'author', 'other'),
                 ('pr', 'head_repository', 'fork/repo'), ('pr', 'base_ref', 'other'),
                 ('fresh_pr', 'head_sha', 'c' * 40), ('fresh_pr', 'base_sha', 'c' * 40),
                 ('fresh_pr', 'draft', True), ('protection', 'enforced', False),
                 ('protection', 'strict', False), ('protection', 'no_bypass', False),
                 ('protection', 'required_checks', []), ('protection', 'read_complete', False),
                 ('protection', 'source', 'unknown')]
        for section, key, value in cases:
            with self.subTest(section=section, key=key):
                s = fixture(); s[section][key] = value; self.rejected(s)

    def test_pagination_and_trees(self):
        cases = [('file_listing', 'all_pages_read', False), ('file_listing', 'has_next_page', True),
                 ('file_listing', 'truncated', True), ('file_listing', 'head_sha', 'c' * 40),
                 ('trees', 'complete', False), ('trees', 'truncated', True),
                 ('trees', 'base_sha', 'c' * 40), ('pr', 'changed_files', 2)]
        for section, key, value in cases:
            with self.subTest(section=section, key=key):
                s = fixture(); s[section][key] = value; self.rejected(s)
        s = fixture(); s['file_listing']['files'] *= 3000; s['pr']['changed_files'] = 3000; self.rejected(s)
        s = fixture(); s['file_listing']['files'] *= 2; s['pr']['changed_files'] = 2; self.rejected(s)

    def test_renames_modes_deletion(self):
        for key, value in [('status', 'removed'), ('status', 'unknown'), ('head_mode', '120000'),
                           ('head_mode', '100755'), ('base_mode', '120000'), ('head_type', 'commit'),
                           ('content_sha', 'c' * 40), ('previous_path', 'scripts/evil.py')]:
            with self.subTest(key=key):
                s = fixture(); s['file_listing']['files'][0][key] = value; self.rejected(s)
        s = fixture(); f = s['file_listing']['files'][0]; f.update(status='renamed', previous_path='scripts/evil.py'); self.rejected(s)
        s = fixture(); f = s['file_listing']['files'][0]; f.update(status='renamed', previous_path='docs/guide/index.md'); self.assertTrue(evaluate(s)['review_candidate'])
        s = fixture(); s['file_listing']['files'][0]['status'] = 'added'; self.rejected(s)
        s['file_listing']['files'][0].update(base_type=None, base_mode=None); self.assertTrue(evaluate(s)['review_candidate'])

    def test_active_content_and_blob_integrity(self):
        for content in [b'<script>alert(1)</script>', b'![x](data:text/html,x)', b'[x](javascript:x)',
                        b'[x](java&#x73;cript:x)', b'%3Cscript%3E',
                        b'<iframe src="x">', b'<img onerror=x>', b'\0', b'\xff', b'a' * 200001]:
            with self.subTest(content=content[:20]):
                s = fixture(); s['file_listing']['files'] = [file_row(content=content)]; self.rejected(s)
        s = fixture(); s['file_listing']['files'][0]['content_base64'] = base64.b64encode(b'changed').decode(); self.rejected(s)
        s = fixture(); s['file_listing']['files'] = [file_row('examples/guide/can/minimal.ini', b'include "../x.ini"')]; self.rejected(s)
        s = fixture(); s['file_listing']['files'] = [file_row('examples/guide/can/minimal.ini', b'workload = "../../../secret.json"')]; self.rejected(s)
        s = fixture(); s['file_listing']['files'] = [file_row('examples/guide/can/minimal.json', b'{"schema_version":2}')]; self.rejected(s)
        s = fixture(); s['file_listing']['files'] = [file_row('docs/guide/assets/guide-minimal-viewer.png', b'<svg/>')]; self.rejected(s)

    def test_evidence_provenance(self):
        cases = [('all_pages_read', False), ('has_next_page', True), ('head_sha', 'c' * 40),
                 ('base_sha', 'c' * 40), ('trusted_workflow_sha', 'c' * 40), ('workflow_path', 'other'),
                 ('repository', 'fork/repo'), ('event', 'push'), ('pull_number', 26),
                 ('samples_reproduced', False), ('public_asset_scan', False), ('images_decoded_reencoded', False),
                 ('no_pr_code_in_privileged_job', False), ('workflow_matches_trusted_main', False)]
        for key, value in cases:
            with self.subTest(key=key):
                s = fixture(); s['evidence'][key] = value; self.rejected(s)
        for key, value in [('conclusion', 'skipped'), ('conclusion', 'failure'), ('status', 'in_progress'),
                           ('run_id', 456), ('run_attempt', 1), ('name', 'spoof')]:
            with self.subTest(key=key, value=value):
                s = fixture(); s['evidence']['checks'][0][key] = value; self.rejected(s)
        s = fixture(); s['evidence']['checks'].append(copy.deepcopy(s['evidence']['checks'][0])); self.rejected(s)

    def test_missing_evidence_stops(self):
        s = fixture()
        for section in list(s):
            with self.subTest(section=section):
                x = copy.deepcopy(s); del x[section]; self.rejected(x)
        self.rejected(None)


if __name__ == '__main__':
    unittest.main()
