#!/usr/bin/env python3
"""Audit rendered href/src destinations in public repository Markdown."""
from __future__ import annotations

import html
import json
import os
import subprocess
import sys
from collections import Counter, defaultdict
from datetime import datetime, timezone
from html.parser import HTMLParser
from pathlib import Path, PurePosixPath
from urllib.parse import unquote, urlsplit

ROOT = Path("/home/hideki/DIR-Simulator-audits/release-v1.1.6-20261017/source").resolve()
OUT = ROOT / "tmp/release-v1.1.6-publication/preflight/markdown-links"
REPORT = OUT / "summary.json"
EXCLUDED_PREFIXES = ("tmp/", "target/", "build/")
LICENSE_RAW_SEGMENT = "licenses"


def git_paths(*args: str) -> set[str]:
    data = subprocess.check_output(["git", "ls-files", "-z", *args], cwd=ROOT)
    return {os.fsdecode(item) for item in data.split(b"\0") if item}


def is_public_markdown(path: str) -> bool:
    p = PurePosixPath(path)
    name = p.name
    root_readme = p.parent == PurePosixPath(".") and name.lower().startswith("readme") and p.suffix.lower() == ".md"
    readme_tree = path.startswith("README/") and p.suffix.lower() == ".md"
    project_docs = path.startswith("docs/") and p.suffix.lower() == ".md"
    examples = path.startswith("examples/") and p.suffix.lower() == ".md"
    return root_readme or readme_tree or project_docs or examples


def exclusion_reason(path: str) -> str | None:
    lowered = path.lower()
    if any(path.startswith(prefix) for prefix in EXCLUDED_PREFIXES):
        return "temporary_or_build_tree"
    parts = {part.lower() for part in PurePosixPath(path).parts}
    if LICENSE_RAW_SEGMENT in parts and "third-party" in parts:
        return "third_party_raw_license"
    if "third-party" in parts and PurePosixPath(path).name.lower().startswith("license"):
        return "third_party_raw_license"
    return None


class AttrCollector(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.links: list[dict[str, object]] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        self._capture(tag, attrs)

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        self._capture(tag, attrs)

    def _capture(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        for name, value in attrs:
            if name.lower() in {"href", "src"} and value is not None:
                self.links.append({
                    "tag": tag.lower(),
                    "attribute": name.lower(),
                    "value": value.strip(),
                    "rendered_html_line": self.getpos()[0],
                })


def source_lines_for(lines: list[str], value: str) -> list[int]:
    parsed = urlsplit(value)
    candidates = {
        value,
        html.unescape(value),
        parsed.path,
        unquote(parsed.path),
        unquote(value),
    }
    candidates = {item for item in candidates if item and item not in {".", "/"}}
    matches: list[int] = []
    for number, line in enumerate(lines, 1):
        if any(candidate in line for candidate in candidates):
            matches.append(number)
    return matches


def resolve_relative(source_rel: str, raw_value: str) -> dict[str, object]:
    parsed = urlsplit(raw_value)
    value = parsed.path
    decoded = unquote(value)
    base = {
        "raw_value": raw_value,
        "fragment": unquote(parsed.fragment) if parsed.fragment else "",
        "query": parsed.query,
    }
    if parsed.scheme:
        if parsed.scheme.lower() in {"http", "https", "mailto", "app"}:
            return {**base, "classification": "absolute_external_or_app"}
        return {**base, "classification": "absolute_uri_scheme", "scheme": parsed.scheme.lower()}
    if parsed.netloc:
        return {**base, "classification": "protocol_relative"}
    if not value:
        if parsed.fragment:
            return {**base, "classification": "same_page_anchor"}
        if parsed.query:
            return {**base, "classification": "same_page_query"}
        return {**base, "classification": "empty_destination"}
    if decoded.startswith("/"):
        return {
            **base,
            "classification": "root_absolute_path",
            "decoded_path": decoded,
        }

    source_path = ROOT / PurePosixPath(source_rel)
    target = (source_path.parent / PurePosixPath(decoded)).resolve(strict=False)
    try:
        target_rel = target.relative_to(ROOT).as_posix()
    except ValueError:
        return {
            **base,
            "classification": "relative_path_outside_repository",
            "decoded_path": decoded,
            "resolved_path": str(target),
        }
    is_source_link = PurePosixPath(decoded).suffix.lower() in {".md", ".markdown"}
    if target.exists():
        kind = "directory" if target.is_dir() else "file" if target.is_file() else "other"
        status = "exists"
    else:
        kind = "directory" if decoded.endswith("/") else "file_or_path"
        status = "missing_directory" if decoded.endswith("/") else "missing_path"
    return {
        **base,
        "classification": "relative_source_link" if is_source_link else "relative_local_path",
        "decoded_path": decoded,
        "resolved_repo_path": target_rel,
        "target_kind": kind,
        "status": status,
    }


def summarize() -> dict[str, object]:
    tracked = git_paths()
    untracked = git_paths("--others", "--exclude-standard")
    candidates = sorted(path for path in tracked | untracked if is_public_markdown(path))
    excluded: Counter[str] = Counter()
    selected: list[str] = []
    missing_sources: list[str] = []
    tracked_count = 0
    untracked_count = 0
    for rel in candidates:
        reason = exclusion_reason(rel)
        if reason:
            excluded[reason] += 1
            continue
        full = ROOT / rel
        if not full.is_file():
            missing_sources.append(rel)
            continue
        selected.append(rel)
        if rel in tracked:
            tracked_count += 1
        else:
            untracked_count += 1

    file_rows: list[dict[str, object]] = []
    all_events: list[dict[str, object]] = []
    parser_errors: list[dict[str, str]] = []
    for rel in selected:
        path = ROOT / rel
        text = path.read_text(encoding="utf-8", errors="replace")
        lines = text.splitlines()
        try:
            import markdown
            rendered = markdown.markdown(text, extensions=["tables", "fenced_code"])
            markdown_version = getattr(markdown, "__version__", "unknown")
        except Exception as exc:
            parser_errors.append({"file": rel, "error": repr(exc)})
            continue
        parser = AttrCollector()
        try:
            parser.feed(rendered)
            parser.close()
        except Exception as exc:
            parser_errors.append({"file": rel, "error": "HTML parse: " + repr(exc)})
            continue
        count = 0
        for found in parser.links:
            raw_value = str(found["value"])
            resolved = resolve_relative(rel, raw_value)
            needs_line_evidence = (
                str(resolved.get("status", "")).startswith("missing")
                or resolved.get("classification") in {
                    "root_absolute_path",
                    "relative_path_outside_repository",
                }
            )
            event = {
                "source": rel,
                "source_lines": source_lines_for(lines, raw_value) if needs_line_evidence else [],
                "tag": found["tag"],
                "attribute": found["attribute"],
                "rendered_html_line": found["rendered_html_line"],
                **resolved,
            }
            all_events.append(event)
            count += 1
        file_rows.append({"file": rel, "tracked": rel in tracked, "href_src_count": count})

    category_counts = Counter(str(event["classification"]) for event in all_events)
    relative_events = [
        event for event in all_events
        if event["classification"] in {"relative_local_path", "relative_source_link"}
    ]
    missing_events = [event for event in relative_events if str(event.get("status", "")).startswith("missing")]
    generated_output_events: list[dict[str, object]] = []
    confirmed_missing_events: list[dict[str, object]] = []
    for event in missing_events:
        source = str(event["source"])
        resolved_repo_path = str(event["resolved_repo_path"])
        output_candidate: Path | None = None
        if source.startswith("docs/guide/") and resolved_repo_path.startswith("docs/guide/"):
            output_candidate = ROOT / "build/guide" / PurePosixPath(resolved_repo_path).relative_to("docs/guide")
        if output_candidate is not None and output_candidate.exists():
            event["generated_mkdocs_output"] = {
                "path": output_candidate.relative_to(ROOT).as_posix(),
                "exists": True,
                "kind": "directory" if output_candidate.is_dir() else "file",
            }
            generated_output_events.append(event)
        else:
            confirmed_missing_events.append(event)
    missing_by_target: dict[str, list[dict[str, object]]] = defaultdict(list)
    for event in confirmed_missing_events:
        missing_by_target[str(event["resolved_repo_path"])].append(event)

    root_absolute = [event for event in all_events if event["classification"] == "root_absolute_path"]
    source_links = [event for event in all_events if event["classification"] == "relative_source_link"]
    same_page_anchors = [event for event in all_events if event["classification"] == "same_page_anchor"]
    outside = [event for event in all_events if event["classification"] == "relative_path_outside_repository"]
    all_fragments = [event for event in all_events if event.get("fragment")]

    root_path_rows = [{
        "source": event["source"],
        "source_lines": event["source_lines"],
        "tag": event["tag"],
        "attribute": event["attribute"],
        "raw_value": event["raw_value"],
        "decoded_path": event["decoded_path"],
    } for event in root_absolute]
    missing_rows = [{
        "source": event["source"],
        "source_lines": event["source_lines"],
        "tag": event["tag"],
        "attribute": event["attribute"],
        "raw_value": event["raw_value"],
        "decoded_path": event["decoded_path"],
        "resolved_repo_path": event["resolved_repo_path"],
        "target_kind": event["target_kind"],
        "status": event["status"],
        "generated_mkdocs_output": event.get("generated_mkdocs_output"),
    } for event in missing_events]
    source_link_rows = [{
        "source": event["source"],
        "source_lines": event["source_lines"],
        "raw_value": event["raw_value"],
        "resolved_repo_path": event.get("resolved_repo_path"),
        "status": event.get("status"),
        "fragment": event.get("fragment", ""),
    } for event in source_links]
    anchor_rows = [{
        "source": event["source"],
        "source_lines": event["source_lines"],
        "tag": event["tag"],
        "attribute": event["attribute"],
        "raw_value": event["raw_value"],
        "classification": event["classification"],
        "fragment": event.get("fragment", ""),
    } for event in all_fragments]

    try:
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        status = subprocess.check_output(
            ["git", "status", "--porcelain", "--untracked-files=all"], cwd=ROOT
        )
        status_sha256 = __import__("hashlib").sha256(status).hexdigest()
    except Exception as exc:
        head = None
        status_sha256 = None
        parser_errors.append({"file": "<git-state>", "error": repr(exc)})

    import markdown
    return {
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "repository_root": str(ROOT),
        "git_head": head,
        "git_status_sha256": status_sha256,
        "python": sys.version.split()[0],
        "markdown_version": getattr(markdown, "__version__", "unknown"),
        "parser": "Python-Markdown to rendered HTML, then html.parser attributes href/src",
        "scope": {
            "includes": ["root README*.md", "README/**/*.md", "docs/**/*.md", "examples/**/*.md"],
            "tracked_files_included": tracked_count,
            "untracked_nonignored_files_included": untracked_count,
            "markdown_files_parsed": len(file_rows),
            "source_files_missing_on_disk": missing_sources,
            "exclusions": dict(excluded),
            "generated_markdown_under_docs_included": True,
            "ignored_or_generated_trees_excluded": ["tmp/", "target/", "build/"],
            "third_party_raw_license_texts_excluded": True,
        },
        "links": {
            "rendered_href_src_attributes": len(all_events),
            "classifications": dict(sorted(category_counts.items())),
            "relative_local_targets_checked": len(relative_events),
            "relative_local_targets_existing": sum(event.get("status") == "exists" for event in relative_events),
            "relative_local_targets_missing": len(missing_events),
            "confirmed_missing_local_targets": len(confirmed_missing_events),
            "unique_confirmed_missing_targets": len(missing_by_target),
            "source_missing_but_generated_mkdocs_output_exists": len(generated_output_events),
            "missing_local_paths": missing_rows,
            "root_absolute_paths_not_resolved": root_path_rows,
            "source_markdown_links": {
                "count": len(source_link_rows),
                "existing": sum(row["status"] == "exists" for row in source_link_rows),
                "missing": sum(str(row["status"]).startswith("missing") for row in source_link_rows),
                "items": source_link_rows,
            },
            "anchors_not_validated": {
                "same_page_anchor_count": len(same_page_anchors),
                "all_link_fragments_count": len(all_fragments),
                "same_page_anchors": anchor_rows,
                "contract": "File existence only; fragment/heading anchor validity is outside this scan.",
            },
            "relative_paths_outside_repository": [{
                "source": event["source"],
                "source_lines": event["source_lines"],
                "raw_value": event["raw_value"],
                "decoded_path": event["decoded_path"],
                "resolved_path": event["resolved_path"],
            } for event in outside],
            "absolute_http_mailto_app_not_browsed": category_counts.get("absolute_external_or_app", 0),
            "other_uri_schemes_not_resolved": category_counts.get("absolute_uri_scheme", 0),
            "protocol_relative_not_browsed": category_counts.get("protocol_relative", 0),
        },
        "files": file_rows,
        "parser_errors": parser_errors,
        "result": "PASS" if not confirmed_missing_events and not parser_errors and not missing_sources else "FAIL",
    }


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    report = summarize()
    REPORT.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    brief = {
        "result": report["result"],
        "markdown_files_parsed": report["scope"]["markdown_files_parsed"],
        "tracked": report["scope"]["tracked_files_included"],
        "untracked": report["scope"]["untracked_nonignored_files_included"],
        "href_src_attributes": report["links"]["rendered_href_src_attributes"],
        "relative_targets_checked": report["links"]["relative_local_targets_checked"],
        "source_missing_paths": report["links"]["relative_local_targets_missing"],
        "confirmed_missing_paths": report["links"]["confirmed_missing_local_targets"],
        "root_absolute_paths": len(report["links"]["root_absolute_paths_not_resolved"]),
        "source_links": report["links"]["source_markdown_links"]["count"],
        "same_page_anchors": report["links"]["anchors_not_validated"]["same_page_anchor_count"],
        "parser_errors": len(report["parser_errors"]),
        "report": str(REPORT),
    }
    print(json.dumps(brief, ensure_ascii=False, indent=2))
    return 0 if report["result"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
