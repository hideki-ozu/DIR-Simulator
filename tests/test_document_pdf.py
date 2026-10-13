"""Check PDF input provenance without launching a renderer in unit tests."""
import importlib.util
from html import escape
from html.parser import HTMLParser
from pathlib import Path
import subprocess
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / 'scripts/export_document_pdf.py'
spec = importlib.util.spec_from_file_location('document_pdf', MODULE)
pdf = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pdf)


class DocumentPdfTests(unittest.TestCase):
    def test_relative_links_preserve_queries_and_fragments(self):
        class Links(HTMLParser):
            def __init__(self):
                super().__init__()
                self.hrefs = []

            def handle_starttag(self, tag, attrs):
                if tag == 'a':
                    self.hrefs.append(dict(attrs)['href'])

        commit = '1' * 40
        cases = [
            ('guide.md?plain=1#section', 'docs/guide.md?plain=1#section'),
            ('../README.md?plain=1&ref=a%2Fb&ref=two#entry',
             'README.md?plain=1&ref=a%2Fb&ref=two#entry'),
            ('guide.md?title=%E6%97%A5%E6%9C%AC%E8%AA%9E&mode=plain',
             'docs/guide.md?title=%E6%97%A5%E6%9C%AC%E8%AA%9E&mode=plain'),
            ('guide.md#section', 'docs/guide.md#section'),
            ('guide.md?plain=1#%E6%97%A5%E6%9C%AC%E8%AA%9E',
             'docs/guide.md?plain=1#%E6%97%A5%E6%9C%AC%E8%AA%9E'),
            ('guide.md#日本語', 'docs/guide.md#%E6%97%A5%E6%9C%AC%E8%AA%9E'),
        ]
        for target, expected in cases:
            with self.subTest(target=target):
                renderer = pdf.Resources('docs/document.md', commit, {})
                renderer.feed(f'<a href="{escape(target, quote=True)}">link</a>')
                links = Links()
                links.feed(''.join(renderer.parts))
                self.assertEqual(links.hrefs,
                                 [f'https://github.com/hideki-ozu/DIR-Simulator/blob/{commit}/{expected}'])

    def test_images_use_the_selected_commit_even_with_dirty_working_files(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for args in [('init', '-q'), ('config', 'user.name', 'PDF test'),
                         ('config', 'user.email', 'pdf-test@example.invalid')]:
                subprocess.run(['git', '-C', folder, *args], check=True)
            (root / 'docs/images').mkdir(parents=True)
            image = root / 'docs/images/図.png'
            image.write_bytes(b'committed image bytes')
            subprocess.run(['git', '-C', folder, 'add', '.'], check=True)
            subprocess.run(['git', '-C', folder, 'commit', '-qm', 'fixed image'], check=True)
            commit = subprocess.check_output(['git', '-C', folder, 'rev-parse', 'HEAD']).decode().strip()
            image.write_bytes(b'uncommitted replacement')
            old = pdf.ROOT
            pdf.ROOT = root
            try:
                inputs = {}
                renderer = pdf.Resources('docs/document.md', commit, inputs)
                renderer.feed('<img src="images/%E5%9B%B3.png"><a href="../README.md#entry">entry</a>')
            finally:
                pdf.ROOT = old
            self.assertEqual(inputs['docs/images/図.png']['sha256'], pdf.digest(b'committed image bytes'))
            self.assertIn('Y29tbWl0dGVkIGltYWdlIGJ5dGVz', ''.join(renderer.parts))
            self.assertIn(f'/blob/{commit}/README.md#entry', ''.join(renderer.parts))

    def test_external_and_escaping_images_are_rejected_without_fetching(self):
        for source in ['https://example.invalid/image.png', '//example.invalid/a.png',
                       '../../../outside.png', '%2Fetc/image.png', 'images\\outside.png']:
            with self.subTest(source=source), self.assertRaises(ValueError):
                renderer = pdf.Resources('docs/document.md', 'unused', {})
                renderer.feed(f'<img src="{source}">')

    def test_existing_artifacts_are_never_overwritten(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / 'kept.pdf'
            output.write_bytes(b'previous review')
            result = subprocess.run(['python3', str(MODULE), 'docs/要件定義書.md',
                                     '--output', str(output)], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b'already exists', result.stderr)
            self.assertEqual(output.read_bytes(), b'previous review')


if __name__ == '__main__':
    unittest.main()
