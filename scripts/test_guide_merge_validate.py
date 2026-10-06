import io
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch
import zlib
from PIL import Image
from guide_merge_validate import validate_png, validate_overlay, Stop
from guide_scope_policy import IMAGES


def png():
    out=io.BytesIO();Image.new('RGB',(4,4),(1,2,3)).save(out,format='PNG');return out.getvalue()


class ValidationTests(unittest.TestCase):
    def test_decode_and_reencode(self):
        clean=validate_png(png());self.assertTrue(clean.startswith(b'\x89PNG'))
    def test_png_polyglot_crc_truncation_metadata(self):
        data=png();chunk=b'tEXt'+b'javascript:x';extra=struct.pack('>I',len(chunk)-4)+chunk+struct.pack('>I',zlib.crc32(chunk)&0xffffffff)
        for invalid in [data+b'<script/>', data[:-1], data[:20]+bytes([data[20]^1])+data[21:], data[:-12]+extra+data[-12:], b'<svg/>']:
            with self.subTest(size=len(invalid)),self.assertRaises((Stop,OSError,ValueError)): validate_png(invalid)
    def test_trusted_commands_no_pr_scripts_or_token(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)/'base';root.mkdir();binary=Path(tmp)/'binary';binary.write_bytes(b'fake')
            for name in IMAGES:
                p=root/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(png())
            calls=[]
            def run(argv,**kwargs):
                calls.append((argv,kwargs));return type('Result',(),{'returncode':0})()
            with patch.dict(os.environ,{'GH_TOKEN':'dummy-test-token','GITHUB_TOKEN':'dummy-test-token'}):
                r=validate_overlay(root,{'docs/guide/index.md':b'# Guide\n'},binary,run)
            self.assertEqual(r['status'],'passed');self.assertFalse(r['pr_code_executed']);self.assertEqual(len(calls),4)
            for argv,kw in calls:
                self.assertNotIn('GH_TOKEN',kw['env']);self.assertNotIn('GITHUB_TOKEN',kw['env'])
                self.assertNotEqual(Path(kw['cwd']),root);self.assertNotIn('shell',kw)
    def test_outside_overlay_and_validation_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);binary=root/'binary';binary.write_bytes(b'x')
            with self.assertRaises(Stop): validate_overlay(root,{'scripts/evil.py':b'print(1)'},binary)
    def test_workflow_static_locks_and_permission_boundaries(self):
        import json
        root=Path(__file__).resolve().parents[1]
        policy=json.loads((root/'.github/guide-merge-policy.json').read_text())
        self.assertIs(policy['enabled'],False);self.assertIsNone(policy['activation_record'])
        self.assertNotIn('Guide scoped validation',policy['branch_required_checks'])
        self.assertIn('Guide scoped validation',policy['required_checks'])
        for name in ['guide-scoped-merge.yml','guide-scoped-validation.yml']:
            text=(root/'.github/workflows'/name).read_text()
            self.assertIn('if: ${{ false }}',text);self.assertIn('persist-credentials: false',text)
            self.assertNotIn('pull_request_target:',text)
            self.assertNotIn('pull_request.head.sha',text)
        writer=(root/'.github/workflows/guide-scoped-merge.yml').read_text()
        self.assertIn('ref: ${{ github.sha }}',writer)
        self.assertIn('workflows: [Guide scoped validation, Guide checks and GitHub Pages]',writer)
        self.assertIn('group: guide-scoped-writer',writer)
        self.assertIn('cancel-in-progress: false',writer)
        pages=(root/'.github/workflows/guide-pages.yml').read_text()
        self.assertIn("vars.GUIDE_SCOPED_MERGE_ENABLED == 'enabled'",pages)
        self.assertIn('inputs.expected_sha == github.sha',pages)


if __name__=='__main__': unittest.main()
