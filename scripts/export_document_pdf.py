"""Export a committed Markdown document and local images as a review PDF.

Requires the existing guide Python dependencies and Playwright/Chromium.
The working tree is never used as the document or image source.
"""
import argparse
import base64
from datetime import datetime, timezone
import hashlib
from html import escape
from html.parser import HTMLParser
import json
import posixpath
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import quote, unquote, urlsplit

ROOT = Path(__file__).resolve().parents[1]
IMAGE_TYPES = {'.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg',
               '.svg': 'image/svg+xml', '.gif': 'image/gif', '.webp': 'image/webp'}
PRINT = r"""
const fs = require('node:fs');
const {chromium} = require(process.env.DOC_PLAYWRIGHT || 'playwright');
(async () => {
  const browser = await chromium.launch({headless: true});
  try {
    const page = await browser.newPage({javaScriptEnabled: false});
    const blocked = [];
    await page.route('**/*', route => {blocked.push(route.request().url()); return route.abort();});
    await page.setContent(fs.readFileSync(process.argv[1], 'utf8'), {waitUntil: 'load'});
    await page.evaluate(() => document.fonts.ready);
    const missing = await page.locator('img').evaluateAll(images =>
      images.filter(image => !image.complete || !image.naturalWidth).length);
    if (blocked.length || missing) throw Error('Unsupported external resource or missing image');
    await page.pdf({path: process.argv[2], format: 'A4', printBackground: true,
      margin: {top: '15mm', bottom: '15mm', left: '12mm', right: '12mm'}});
    console.log(JSON.stringify({playwright: require((process.env.DOC_PLAYWRIGHT || 'playwright') + '/package.json').version,
      chromium: browser.version(), images: await page.locator('img').count(), missing_images: missing,
      external_requests: blocked.length}));
  } finally {await browser.close();}
})().catch(error => {console.error(error.message); process.exit(1);});
"""


def git(*args):
    return subprocess.check_output(['git', '-C', str(ROOT), *args])


def digest(data):
    return hashlib.sha256(data).hexdigest()


def local_path(source, target):
    path = posixpath.normpath(posixpath.join(posixpath.dirname(source), unquote(target)))
    if path.startswith(('/', '../')) or path == '..' or '\\' in path:
        raise ValueError('Resource path must stay inside the repository')
    return path


class Resources(HTMLParser):
    def __init__(self, source, commit, inputs):
        super().__init__(convert_charrefs=False)
        self.source, self.commit, self.inputs = source, commit, inputs
        self.parts = []

    def start(self, tag, attrs, closed=False):
        updated = []
        for name, value in attrs:
            if value is not None and (tag, name) in [('img', 'src'), ('a', 'href')]:
                url = urlsplit(value)
                if tag == 'img':
                    if url.scheme or url.netloc or not url.path:
                        raise ValueError('Images must be committed local files')
                    path = local_path(self.source, url.path)
                    kind = IMAGE_TYPES.get(Path(path).suffix.lower())
                    if kind is None:
                        raise ValueError('Unsupported image type')
                    data = git('show', f'{self.commit}:{path}')
                    self.inputs[path] = {'sha256': digest(data), 'bytes': len(data)}
                    value = f'data:{kind};base64,' + base64.b64encode(data).decode('ascii')
                elif not url.scheme and not url.netloc and url.path:
                    path = local_path(self.source, url.path)
                    value = f'https://github.com/hideki-ozu/DIR-Simulator/blob/{self.commit}/' + quote(path)
                    if url.query:
                        value += '?' + url.query
                    if url.fragment:
                        value += '#' + quote(url.fragment, safe='/%')
            updated.append(name if value is None else f'{name}="{escape(value, quote=True)}"')
        self.parts.append('<' + tag + ((' ' + ' '.join(updated)) if updated else '') + (' />' if closed else '>'))

    def handle_starttag(self, tag, attrs):
        self.start(tag, attrs)

    def handle_startendtag(self, tag, attrs):
        self.start(tag, attrs, True)

    def handle_endtag(self, tag):
        self.parts.append(f'</{tag}>')

    def handle_data(self, data):
        self.parts.append(data)

    def handle_entityref(self, name):
        self.parts.append(f'&{name};')

    def handle_charref(self, name):
        self.parts.append(f'&#{name};')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', help='Repository-relative Markdown path')
    parser.add_argument('--revision', default='HEAD', help='Committed source revision or tag')
    parser.add_argument('--output', type=Path, required=True, help='New PDF path; existing files are rejected')
    args = parser.parse_args()
    source = local_path('root.md', args.source)
    if Path(source).suffix != '.md':
        parser.error('source must be a Markdown file')
    output = args.output.resolve()
    evidence = output.with_suffix('.pdf.json')
    if output.exists() or evidence.exists():
        parser.error('output or evidence already exists')
    import markdown
    commit = git('rev-parse', '--verify', '--end-of-options', args.revision + '^{commit}').decode().strip()
    raw = git('show', f'{commit}:{source}')
    inputs = {source: {'sha256': digest(raw), 'bytes': len(raw)}}
    resources = Resources(source, commit, inputs)
    resources.feed(markdown.markdown(raw.decode('utf-8'), extensions=['tables', 'fenced_code', 'toc']))
    generated = datetime.now(timezone.utc).isoformat(timespec='seconds')
    content = ''.join(resources.parts)
    html = f'''<!doctype html><html lang="ja"><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'">
<style>body{{font-family:"Noto Sans CJK JP",sans-serif;font-size:9pt;line-height:1.6}}
table{{border-collapse:collapse;width:100%;table-layout:fixed}}th,td{{border:1px solid #aaa;padding:4px;overflow-wrap:anywhere}}
img{{max-width:100%;height:auto}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;font-size:8pt}}
h1,h2,h3{{break-after:avoid}}.provenance{{font-size:8pt;overflow-wrap:anywhere;border-bottom:1px solid #aaa;padding-bottom:8px}}</style>
<body><div class="provenance">生成日（UTC）：{generated}<br>対象コミット：{commit}<br>編集元：{escape(source)}</div>{content}</body></html>'''
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='dir-document-pdf-') as folder:
        page, pdf = Path(folder) / 'document.html', Path(folder) / 'document.pdf'
        page.write_text(html, encoding='utf-8')
        measured = json.loads(subprocess.check_output(['node', '-e', PRINT, str(page), str(pdf)], timeout=60))
        data = pdf.read_bytes()
        if not data.startswith(b'%PDF-'):
            raise ValueError('Renderer did not produce a PDF')
        # Exclusive creation avoids silently replacing a review artifact.
        with output.open('xb') as file:
            file.write(data)
    report = {'schema_version': 1, 'source_commit': commit, 'generated_at_utc': generated,
              'source_inputs': inputs, 'python_markdown': markdown.__version__, 'renderer': measured,
              'pdf': {'sha256': digest(data), 'bytes': len(data)},
              'visual_review': 'required_before_distribution'}
    with evidence.open('x', encoding='utf-8') as file:
        json.dump(report, file, ensure_ascii=False, indent=2)
        file.write('\n')
    print(f'Created {output} and {evidence}; visual review remains required.')


if __name__ == '__main__':
    main()
