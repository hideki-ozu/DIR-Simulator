"""Compare current nav, generated HTML and the selected Ethernet site report."""
from pathlib import Path
import re,json
ROOT=Path(__file__).resolve().parents[1]
def main():
 nav=re.findall(r'^\s+- [^\n]+: ([^\n]+\.md)\s*$',(ROOT/'mkdocs.yml').read_text(),re.M)
 pages=json.loads((ROOT/'docs/verification/results/ethernet-multicast/site-browser.json').read_text())['pages']
 assert {Path(x).stem for x in nav}=={Path(x).stem for x in pages}
 html=list((ROOT/'build/guide').glob('*.html'));assert len(html)==len(nav)+1 and (ROOT/'build/guide/404.html').is_file()
 text=(ROOT/'docs/ガイド作成・統合手順.md').read_text();assert f'navは{len(nav)}ページ（{len(nav)-2}記事＋入口・ライセンス表示の2ページ）' in text and f'生成HTMLは404込み{len(html)}件' in text
 print(f'PASS: nav/browser {len(nav)} paths identical, articles {len(nav)-2}+index/license2, HTML including404 {len(html)}')
if __name__=='__main__':main()
