"""Check generated local links, anchors and image paths without network access."""
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlparse
import json
import yaml

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / 'build/guide'
SITE_PREFIX = urlparse(yaml.load((ROOT / 'mkdocs.yml').read_text(encoding='utf-8'), Loader=yaml.BaseLoader)['site_url']).path


class Page(HTMLParser):
    def __init__(self):
        super().__init__()
        self.ids = set()
        self.links = []

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if 'id' in attrs:
            self.ids.add(attrs['id'])
        for attr in ['href', 'src']:
            if attr in attrs:
                self.links.append(attrs[attr])


def main():
    pages = {}
    for file in SITE.rglob('*.html'):
        page = Page()
        page.feed(file.read_text(encoding='utf-8'))
        pages[file.resolve()] = page
    checked = 0
    for file, page in pages.items():
        for link in page.links:
            parsed = urlparse(link)
            if parsed.scheme or parsed.netloc:
                continue
            relative = unquote(parsed.path)
            if relative.startswith(SITE_PREFIX):
                target = SITE / relative[len(SITE_PREFIX):]
            elif relative.startswith('/'):
                raise AssertionError(f'Unexpected site-root link: {link}')
            else:
                target = file.parent / relative if relative else file
            if target.is_dir():
                target /= 'index.html'
            target = target.resolve()
            assert target.is_relative_to(SITE.resolve()), f'Link escapes {SITE_PREFIX}: {link}'
            assert target.exists(), f'Missing target in {file.name}: {link}'
            if parsed.fragment and target in pages:
                assert unquote(parsed.fragment) in pages[target].ids, f'Missing anchor: {link}'
            checked += 1
    # Only the generated guide is published, never the workspace/evidence tree.
    allowed = {'.html', '.css', '.js', '.json', '.txt', '.png', '.zip', '.gz',
               '.svg', '.woff', '.woff2', '.ttf', '.ico', '.map', '.xml'}
    public_files = 0
    for file in SITE.rglob('*'):
        if not file.is_file():
            continue
        assert not file.is_symlink(), f'Symlink in public artifact: {file.name}'
        assert file.suffix in allowed, f'Unexpected public asset: {file.name}'
        if file.suffix in {'.html', '.css', '.js', '.json', '.txt', '.xml', '.map'}:
            text = file.read_text(encoding='utf-8')
            for private in ['C:\\Users\\', '/mnt/c/Users/', '/home/hideki/', '.chatgpt.site/']:
                assert private not in text, f'Private path/host in public asset: {file.name}'
        public_files += 1
    report = {'status': 'passed', 'html_files': len(pages), 'local_links_and_assets': checked,
              'site_prefix': SITE_PREFIX, 'public_assets_checked': public_files}
    output = ROOT / 'guide-evidence/link-verification.json'
    output.parent.mkdir(exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(f'PASS: {len(pages)} HTML files, {checked} local links/assets/anchors.')


if __name__ == '__main__':
    main()
