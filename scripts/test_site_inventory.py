"""Exercise current inventory strictness and historical browser evidence boundaries."""

import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from site_inventory import check_site_inventory, current_inventory, historical_inventory


class SiteInventoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.nav = ["index.md", "article.md", "reference/article.md"]
        self.write_config()
        for page in self.nav:
            self.write("docs/guide/" + page)
            self.write("build/guide/" + str(Path(page).with_suffix(".html")))
        self.write("build/guide/404.html")
        self.report = {
            "mode": "site", "status": "passed",
            "pages": ["index.html", "article.html"],
            "errors": [], "failures": [], "external": [],
            "width": {"document": 390, "viewport": 390},
            "searches": {"article": ["Article"]},
        }
        self.report_path = "docs/verification/results/fixture/site-browser.json"
        self.write(self.report_path, json.dumps(self.report))

    def write(self, relative, content="fixture"):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")

    def write_config(self):
        self.write("mkdocs.yml", "docs_dir: docs/guide\nsite_dir: build/guide\n"
                   "use_directory_urls: false\nnav:\n"
                   + "".join(f"  - Page: {page}\n" for page in self.nav)
                   + "markdown_extensions:\n  - toc:\n"
                   "      slugify: !!python/name:markdown.extensions.toc.slugify_unicode\n")

    def test_nested_paths_and_historical_subset_pass_without_modifying_report(self):
        before = (self.root / self.report_path).read_bytes()
        check_site_inventory(self.root, "fixture", "article.md")
        current, count = current_inventory(self.root)
        self.assertEqual(current, {"index.html", "article.html", "reference/article.html"})
        self.assertEqual(count, 4)
        self.assertEqual((self.root / self.report_path).read_bytes(), before)

    def test_missing_current_html_fails(self):
        (self.root / "build/guide/reference/article.html").unlink()
        with self.assertRaisesRegex(AssertionError, "missing=.*reference/article.html"):
            current_inventory(self.root)

    def test_extra_nested_html_fails_even_with_same_basename(self):
        self.write("build/guide/other/article.html")
        with self.assertRaisesRegex(AssertionError, "extra=.*other/article.html"):
            current_inventory(self.root)

    def test_missing_source_and_error_page_fail(self):
        for relative, message in (
            ("docs/guide/reference/article.md", "Missing nav Markdown"),
            ("build/guide/404.html", "Missing generated 404"),
        ):
            with self.subTest(relative=relative):
                path = self.root / relative
                path.unlink()
                with self.assertRaisesRegex(AssertionError, message):
                    current_inventory(self.root)
                self.write(relative)

    def test_duplicate_nav_path_fails(self):
        self.nav.append("article.md")
        self.write_config()
        with self.assertRaisesRegex(AssertionError, "Duplicate nav"):
            current_inventory(self.root)

    def test_historical_duplicate_extra_and_wrong_directory_fail(self):
        current, _ = current_inventory(self.root)
        for pages, message in (
            (["article.html", "article.html"], "Duplicate historical"),
            (["article.html", "removed.html"], "Historical pages absent"),
            (["index.html", "reference/article.html"], "did not browse its article"),
        ):
            with self.subTest(pages=pages):
                report = dict(self.report, pages=pages)
                with self.assertRaisesRegex(AssertionError, message):
                    historical_inventory(report, current, "article.md")

    def test_historical_failed_or_incomplete_checks_are_rejected(self):
        current, _ = current_inventory(self.root)
        changes = [
            {"status": "failed"}, {"mode": "viewer"},
            {"errors": ["pageerror"]}, {"failures": ["404"]},
            {"external": ["https://example.invalid/"]}, {"failures": None},
            {"width": {"document": 500, "viewport": 390}},
            {"searches": {"article": []}},
            {"search_hits": {"article": False}},
            {"comparison_table": {"width": 366, "scrollWidth": 500}},
        ]
        for change in changes:
            with self.subTest(change=change):
                report = copy.deepcopy(self.report)
                report.update(change)
                with self.assertRaises(AssertionError):
                    historical_inventory(report, current, "article.md")

    def test_noncanonical_paths_are_rejected_in_current_and_historical_inventory(self):
        current, _ = current_inventory(self.root)
        for path in ("../article.md", "/article.md", "reference//article.md", "reference\\article.md"):
            with self.subTest(path=path):
                self.nav = [path]
                self.write_config()
                with self.assertRaisesRegex(AssertionError, "canonical relative"):
                    current_inventory(self.root)
                report = dict(self.report, pages=[path.replace(".md", ".html")])
                with self.assertRaisesRegex(AssertionError, "canonical relative"):
                    historical_inventory(report, current, "article.md")

    def test_failure_remains_failure_with_python_optimization(self):
        (self.root / "build/guide/article.html").unlink()
        result = subprocess.run(
            [sys.executable, "-O", "-c",
             "from pathlib import Path; from site_inventory import current_inventory; "
             "import sys; current_inventory(Path(sys.argv[1]))", str(self.root)],
            cwd=Path(__file__).parent, text=True, capture_output=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Current nav/generated HTML mismatch", result.stderr)


if __name__ == "__main__":
    unittest.main()
