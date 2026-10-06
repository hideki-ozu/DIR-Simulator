"""Export review copies with absolute links. Does not publish to Qiita."""
import argparse
import re
from pathlib import Path
from urllib.parse import quote, urljoin, urlparse

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--base-url', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if urlparse(args.base_url).scheme not in ('http', 'https'):
        parser.error('--base-url must be an HTTP(S) public guide URL')
    args.output.mkdir(parents=True, exist_ok=True)

    def replace(match):
        label, target = match.groups()
        if urlparse(target).scheme or target.startswith('#'):
            return match.group(0)
        path, sep, fragment = target.partition('#')
        if path.endswith('.md'):
            path = path[:-3] + '.html'
        target = urljoin(args.base_url.rstrip('/') + '/', quote(path))
        if sep:
            target += '#' + quote(fragment)
        return f'[{label}]({target})'

    for source in sorted((ROOT / 'docs/guide').glob('*.md')):
        text = source.read_text(encoding='utf-8')
        # Keep illustrative code fences literal; only rewrite actual prose links.
        parts = re.split(r'(^```[^\n]*\n.*?^```\s*$)', text,
                         flags=re.MULTILINE | re.DOTALL)
        for i in range(0, len(parts), 2):
            parts[i] = re.sub(r'\[([^\]]*)\]\(([^)]+)\)', replace, parts[i])
        (args.output / source.name).write_text(''.join(parts), encoding='utf-8')
    print(f'Exported {len(list((ROOT / "docs/guide").glob("*.md")))} review copies; no publication performed.')


if __name__ == '__main__':
    main()
