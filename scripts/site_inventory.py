"""Validate the current guide inventory separately from historical browser evidence."""

import json
from pathlib import PurePosixPath

import yaml


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def relative_page(value, suffix):
    require(isinstance(value, str) and value, f"Invalid page path: {value!r}")
    path = PurePosixPath(value)
    require(
        not path.is_absolute()
        and "\\" not in value
        and ".." not in path.parts
        and path.as_posix() == value
        and path.suffix == suffix,
        f"Expected canonical relative {suffix} page path: {value!r}",
    )
    return path


def nav_pages(nav):
    """Flatten MkDocs sections without losing directories or duplicate entries."""
    if isinstance(nav, str):
        return [str(relative_page(nav, ".md"))]
    if isinstance(nav, list):
        return [page for entry in nav for page in nav_pages(entry)]
    if isinstance(nav, dict):
        return [page for entry in nav.values() for page in nav_pages(entry)]
    raise AssertionError(f"Invalid MkDocs nav entry: {nav!r}")


def unique_pages(pages, label, suffix):
    require(isinstance(pages, list) and pages, f"{label} must contain pages")
    canonical = [str(relative_page(page, suffix)) for page in pages]
    require(len(canonical) == len(set(canonical)), f"Duplicate {label} page paths")
    return set(canonical)


def current_inventory(root):
    # BaseLoader accepts MkDocs' Python-name slugify tag without executing it.
    config = yaml.load((root / "mkdocs.yml").read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
    require(config.get("use_directory_urls") == "false", "Inventory requires use_directory_urls: false")
    sources = unique_pages(nav_pages(config.get("nav")), "nav", ".md")
    docs = root / config["docs_dir"]
    missing_sources = sorted(page for page in sources if not (docs / page).is_file())
    require(not missing_sources, f"Missing nav Markdown sources: {missing_sources}")
    expected = {str(PurePosixPath(page).with_suffix(".html")) for page in sources}
    require("404.html" not in expected, "404.html is reserved for the generated error page")
    site = root / config["site_dir"]
    require((site / "404.html").is_file(), "Missing generated 404.html")
    generated = {page.relative_to(site).as_posix() for page in site.rglob("*.html") if page.is_file()}
    actual = generated - {"404.html"}
    require(
        actual == expected,
        f"Current nav/generated HTML mismatch: missing={sorted(expected - actual)}, "
        f"extra={sorted(actual - expected)}",
    )
    return expected, len(generated)


def historical_inventory(report, current, article):
    """Check a recorded run, without claiming that it browsed later nav additions."""
    require(isinstance(report, dict), "Historical browser report must be an object")
    require(report.get("mode") == "site", "Historical browser report is not a site run")
    require(report.get("status") == "passed", "Historical browser report did not pass")
    for field in ("errors", "failures", "external"):
        require(report.get(field) == [], f"Historical browser report has missing/nonempty {field}")
    pages = unique_pages(report.get("pages"), "historical browser", ".html")
    require(pages <= current, f"Historical pages absent from current nav: {sorted(pages - current)}")
    article_html = str(relative_page(article, ".md").with_suffix(".html"))
    require(article_html in pages, f"Historical report did not browse its article: {article_html}")
    width = report.get("width", {})
    require(
        isinstance(width, dict)
        and type(width.get("viewport")) in (int, float)
        and type(width.get("document")) in (int, float)
        and width["viewport"] > 0
        and 0 < width["document"] <= width["viewport"] + 1,
        "Historical browser report has invalid/overflowing viewport width",
    )
    searches = report.get("searches")
    require(
        isinstance(searches, dict)
        and searches
        and all(isinstance(term, str) and term and isinstance(hits, list) and hits
                and all(isinstance(hit, str) and hit for hit in hits)
                for term, hits in searches.items()),
        "Historical browser report has missing/empty search results",
    )
    if "search_hits" in report:
        hits = report["search_hits"]
        require(isinstance(hits, dict) and hits.keys() == searches.keys()
                and all(hit is True for hit in hits.values()),
                "Historical browser report has unsuccessful article search hits")
    if "comparison_table" in report:
        table = report["comparison_table"]
        require(isinstance(table, dict)
                and type(table.get("width")) in (int, float)
                and type(table.get("scrollWidth")) in (int, float)
                and table["width"] > 0
                and 0 < table["scrollWidth"] <= table["width"] + 1,
                "Historical browser report has invalid/overflowing comparison table")
    return len(pages)


def check_site_inventory(root, report_name, article):
    current, generated_count = current_inventory(root)
    report_path = root / "docs/verification/results" / report_name / "site-browser.json"
    report = json.loads(report_path.read_text(encoding="utf-8"))
    historical_count = historical_inventory(report, current, article)
    print(
        f"PASS: current nav/generated HTML {len(current)} relative paths identical "
        f"({generated_count} HTML including404); historical {report_name} browser report "
        f"{historical_count} pages passed, subset of current nav with its article. "
        "Historical report does not validate later page additions."
    )
