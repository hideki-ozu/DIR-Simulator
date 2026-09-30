import html.parser
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "scripts" / "generate_traceability.py"


class TableParser(html.parser.HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.tables = []
        self.current_table = None
        self.current_group = None
        self.current_row = None
        self.current_cell = None

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == "table":
            self.current_table = {"attrs": attrs, "rows": []}
            self.tables.append(self.current_table)
        elif tag == "tbody" and self.current_table is not None:
            self.current_group = attrs
        elif tag == "tr" and self.current_table is not None:
            self.current_row = {"attrs": attrs, "cells": [], "group": self.current_group or {}}
        elif tag in ("th", "td") and self.current_row is not None:
            self.current_cell = {"tag": tag, "attrs": attrs, "text": "", "links": []}
            self.current_row["cells"].append(self.current_cell)
        elif tag == "a" and self.current_cell is not None:
            self.current_cell["links"].append(attrs.get("href", ""))

    def handle_endtag(self, tag):
        if tag in ("td", "th"):
            self.current_cell = None
        elif tag == "tr" and self.current_row is not None:
            self.current_table["rows"].append(self.current_row)
            self.current_row = None
        elif tag == "tbody":
            self.current_group = None
        elif tag == "table":
            self.current_table = None

    def handle_data(self, data):
        if self.current_cell is not None:
            self.current_cell["text"] += data


class TraceabilityGeneratorFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.docs = self.root / "docs"
        for directory in ("specs", "design", "verification/cases"):
            (self.docs / directory).mkdir(parents=True, exist_ok=True)
        self.requirements = {}
        self.functions = {}
        self.orphan_functions = set()
        self.hierarchy = {}
        self.nodes = []
        self.exceptions = set()

    def add_requirement(
        self,
        ident,
        title,
        functions,
        acceptance=(),
        *,
        parent=None,
        position="上位",
        decomposition="未分解",
    ):
        self.requirements[ident] = {
            "title": title,
            "functions": list(functions),
            "acceptance": list(acceptance),
        }
        self.hierarchy[ident] = {
            "id": ident,
            "parent": parent,
            "position": position,
            "decomposition": decomposition,
        }

    def add_exception(self, requirement):
        self.exceptions.add(requirement)

    def add_orphan_function(self, function):
        self.orphan_functions.add(function)

    def add_node(self, ident, stage, requirements, upstream, *, pending=(), state="confirmed"):
        self.nodes.append(
            {
                "id": ident,
                "stage": stage,
                "requirements": list(requirements),
                "upstream": list(upstream),
                "state": state,
                "pending": list(pending),
            }
        )

    def write_docs(self):
        tick = chr(96)
        fence = tick * 3
        req_lines = [
            f"文書ID：{tick}requirements{tick}",
            "# 要件定義書",
            "| 要件ID | 名称 | 要件（条件・対象・期待結果） | 検証先・確認内容 | TBD・注釈 |",
            "| --- | --- | --- | --- | --- |",
        ]
        for requirement, data in self.requirements.items():
            req_lines.append(
                f'| <a id="{requirement.lower()}"></a>**{requirement}** | {data["title"]} | body | test | — |'
            )
        req_lines.append("")
        for data in self.requirements.values():
            for ac in data["acceptance"]:
                req_lines.append(f'<a id="{ac.lower()}"></a>**{ac}**')
        for requirement in sorted(self.exceptions):
            req_lines.extend(
                [
                    fence + "trace-exception",
                    json.dumps(
                        {
                            "requirement": requirement,
                            "reason": "scope exception",
                            "stages": ["requirement", "verification"],
                        }
                    ),
                    fence,
                ]
            )
        (self.docs / "要件定義書.md").write_text("\n".join(req_lines) + "\n", encoding="utf-8")
        hierarchy = {
            "schema_version": 1,
            "requirements": list(self.hierarchy.values()),
        }
        (self.docs / "要件階層.json").write_text(
            json.dumps(hierarchy, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )

        for requirement, data in self.requirements.items():
            for function in data["functions"]:
                self.functions[function] = requirement
        function_lines = [f"文書ID：{tick}functions{tick}", "# 機能仕様書"]
        for function in sorted(set(self.functions) | self.orphan_functions):
            function_lines.append(f'<a id="{function.lower()}"></a>**{function}**')
        function_lines.extend(
            [
                "### 11.2. 要件と機能の対応",
                "| 要件 | 概要 | 機能 | 受入条件 |",
                "| --- | --- | --- | --- |",
            ]
        )
        for requirement, data in self.requirements.items():
            functions = "、".join(
                f"[{function}](#{function.lower()})" for function in data["functions"]
            )
            acceptance = "、".join(
                f"[{ac}](#{ac.lower()})" for ac in data["acceptance"]
            )
            function_lines.append(
                f"| [{requirement}](#{requirement.lower()}) | {data['title']} | {functions} | {acceptance} |"
            )
        (self.docs / "機能仕様書.md").write_text("\n".join(function_lines) + "\n", encoding="utf-8")

        grouped = {}
        for node in self.nodes:
            stage = node["stage"]
            if stage == "architecture":
                key = "architecture"
                node["anchor"] = node["id"].split("#", 1)[1]
                node["docid"] = "architecture"
            elif stage == "verification":
                key = node["id"]
                node["anchor"] = node["id"].lower()
                node["docid"] = "verification-" + node["id"][-4:]
            else:
                node["docid"], node["anchor"] = node["id"].split("#", 1)
                key = node["docid"]
            grouped.setdefault((stage, key), []).append(node)
        for (stage, key), nodes in grouped.items():
            if stage == "architecture":
                path = self.docs / "アーキテクチャ設計書.md"
            elif stage == "spec":
                path = self.docs / "specs" / f"{key}.md"
            elif stage == "design":
                path = self.docs / "design" / f"{key}.md"
            else:
                path = self.docs / "verification/cases" / f"{key}.md"
            path.parent.mkdir(parents=True, exist_ok=True)
            lines = [f"文書ID：{tick}{nodes[0]['docid']}{tick}", f"# {key}"]
            for node in nodes:
                lines.append(f'<a id="{node["anchor"]}"></a>')
                trace = {
                    field: node[field]
                    for field in ("id", "stage", "requirements", "upstream", "state", "pending")
                }
                lines.extend([fence + "trace", json.dumps(trace), fence])
            path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    def run_generator(self, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(self.root), *args],
            text=True,
            capture_output=True,
        )

    def generate(self, *, allow_errors=False):
        self.write_docs()
        result = self.run_generator()
        if not allow_errors:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertIn(result.returncode, (0, 1), result.stderr)
        output = self.docs / "要件トレーサビリティ一覧.html"
        self.assertTrue(output.exists())
        return output.read_text(encoding="utf-8")

    def parse_html(self, content):
        parser = TableParser()
        parser.feed(content)
        parser.close()
        return parser

    def route_rows(self, table):
        normalized = []
        spans = {}
        group = None
        for row in table["rows"]:
            if row["group"] != group:
                group = row["group"]
                spans = {}
            if row["attrs"].get("class") == "group-label":
                continue
            raw = iter(row["cells"])
            cells = []
            column = 0
            while column < 8:
                if column in spans:
                    cell, remaining = spans[column]
                    cells.append(cell)
                    if remaining <= 1:
                        del spans[column]
                    else:
                        spans[column] = (cell, remaining - 1)
                    column += 1
                    continue
                try:
                    cell = next(raw)
                except StopIteration:
                    break
                cells.append(cell)
                rowspan = int(cell["attrs"].get("rowspan", "1"))
                if rowspan > 1:
                    spans[column] = (cell, rowspan - 1)
                column += int(cell["attrs"].get("colspan", "1"))
            normalized.append({**row, "cells": cells})
        return normalized


class TestTraceabilityGenerator(TraceabilityGeneratorFixture):
    def test_hierarchy_html_escapes_fields_and_links_each_requirement_and_parent(self):
        self.add_requirement(
            "DIR-REQ-0001",
            "<script>root()</script>",
            ["DIR-FUNC-0001"],
            position="<top>",
            decomposition="root & <split>",
        )
        self.add_requirement(
            "DIR-REQ-0002",
            "Child",
            ["DIR-FUNC-0002"],
            parent="DIR-REQ-0001",
            position="下位",
            decomposition="<script>child()</script>",
        )
        self.generate()

        content = (self.docs / "要件階層.html").read_text(encoding="utf-8")
        requirement_rows = [
            line for line in content.splitlines() if line.startswith('<tr id="dir-req-')
        ]
        self.assertEqual(len(requirement_rows), 2)
        self.assertIn(
            'href="%E8%A6%81%E4%BB%B6%E5%AE%9A%E7%BE%A9%E6%9B%B8.md#dir-req-0001">DIR-REQ-0001',
            content,
        )
        self.assertIn('href="#dir-req-0001">DIR-REQ-0001</a>', content)
        self.assertIn("&lt;script&gt;root()&lt;/script&gt;", content)
        self.assertIn("&lt;top&gt;", content)
        self.assertIn("root &amp; &lt;split&gt;", content)
        self.assertIn("&lt;script&gt;child()&lt;/script&gt;", content)
        self.assertNotIn("<script>", content)

    def test_invalid_hierarchies_fail_before_writing_any_report(self):
        self.add_requirement("DIR-REQ-0001", "Root", ["DIR-FUNC-0001"])
        self.add_requirement(
            "DIR-REQ-0002", "Child", ["DIR-FUNC-0002"], parent="DIR-REQ-0001"
        )
        self.write_docs()
        hierarchy_path = self.docs / "要件階層.json"
        valid = json.loads(hierarchy_path.read_text(encoding="utf-8"))
        root, child = valid["requirements"]
        cases = (
            ("missing", [root], "coverage mismatch"),
            ("duplicate", [root, child, root], "duplicate requirement DIR-REQ-0001"),
            (
                "unknown parent",
                [root, {**child, "parent": "DIR-REQ-0999"}],
                "unknown parent DIR-REQ-0999",
            ),
            (
                "cycle",
                [
                    {**root, "parent": "DIR-REQ-0002"},
                    {**child, "parent": "DIR-REQ-0001"},
                ],
                "hierarchy cycle",
            ),
        )
        outputs = (
            self.docs / "要件トレーサビリティ一覧.md",
            self.docs / "要件トレーサビリティ一覧.html",
            self.docs / "要件階層.html",
        )
        for output in outputs:
            output.write_text(f"existing {output.name}", encoding="utf-8")

        for label, rows, diagnostic in cases:
            with self.subTest(case=label):
                hierarchy_path.write_text(
                    json.dumps(
                        {"schema_version": 1, "requirements": rows},
                        ensure_ascii=False,
                    ),
                    encoding="utf-8",
                )
                result = self.run_generator()
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn(diagnostic, result.stderr)
                for output in outputs:
                    self.assertEqual(
                        output.read_text(encoding="utf-8"), f"existing {output.name}"
                    )

    def test_paths_preserve_branches_reconvergence_scope_and_detached_merge(self):
        self.add_requirement("DIR-REQ-0001", "Root & <branch>", ["DIR-FUNC-0001"], ["DIR-AC-0001"])
        self.add_requirement("DIR-REQ-0002", "Second", ["DIR-FUNC-0002"], ["DIR-AC-0002"])
        self.add_node("spec-shared#shared", "spec", ["DIR-REQ-0001", "DIR-REQ-0002"], ["DIR-FUNC-0001", "DIR-FUNC-0002"])
        self.add_node("spec-side#side", "spec", ["DIR-REQ-0001"], ["DIR-FUNC-0001"])
        self.add_node("architecture#left", "architecture", ["DIR-REQ-0001"], ["spec-shared#shared"])
        self.add_node("architecture#right", "architecture", ["DIR-REQ-0001"], ["spec-shared#shared"])
        self.add_node("architecture#side", "architecture", ["DIR-REQ-0001"], ["spec-side#side"])
        self.add_node("architecture#second", "architecture", ["DIR-REQ-0002"], ["spec-shared#shared"])
        self.add_node("design-merge#merge", "design", ["DIR-REQ-0001"], ["architecture#left", "architecture#right", "architecture#detached"])
        self.add_node("design-side#side", "design", ["DIR-REQ-0001"], ["architecture#side"])
        self.add_node("DIR-TEST-0001", "verification", ["DIR-REQ-0001"], ["design-merge#merge"])
        self.add_node("architecture#detached", "architecture", ["DIR-REQ-0001"], [], pending=["awaiting allocation"], state="draft")
        html_text = self.generate()
        parser = self.parse_html(html_text)
        route_tables = [
            table for table in parser.tables
            if "trace-table" in table["attrs"].get("class", "")
        ]
        req1_table = next(table for table in route_tables if "DIR-REQ-0001" in str(table))
        req2_table = next(table for table in route_tables if "DIR-REQ-0002" in str(table))
        req1_rows = [row for row in self.route_rows(req1_table) if row["group"].get("data-requirement") == "DIR-REQ-0001"]
        req2_rows = [row for row in self.route_rows(req2_table) if row["group"].get("data-requirement") == "DIR-REQ-0002"]
        self.assertEqual(len(req1_rows), 4)
        self.assertEqual(len(req2_rows), 1)
        all_route_rows = [row for table in route_tables for row in self.route_rows(table)]
        self.assertTrue(all(len(row["cells"]) == 8 for row in all_route_rows))
        self.assertTrue(
            any(
                int(cell["attrs"].get("rowspan", "1")) > 1
                for table in route_tables
                for row in table["rows"]
                for cell in row["cells"]
            )
        )
        self.assertTrue(
            all(len(cell["links"]) <= 1 for row in all_route_rows for cell in row["cells"][2:6])
        )

        merge_rows = [row for row in req1_rows if "design-merge#merge" in row["cells"][4]["text"]]
        self.assertEqual(len(merge_rows), 3)
        self.assertTrue(all("rowspan" not in row["cells"][4]["attrs"] for row in merge_rows))
        self.assertEqual(
            {row["cells"][3]["text"].strip() for row in merge_rows},
            {"architecture#left（確定）", "architecture#right（確定）", "architecture#detached（草案）"},
        )
        detached_merge = next(row for row in merge_rows if row["group"].get("class") == "group-detached")
        self.assertIn("DIR-TEST-0001", detached_merge["cells"][5]["text"])
        self.assertIn("未接続（宣言対象）", detached_merge["cells"][6]["text"])
        self.assertEqual(req2_rows[0]["cells"][2]["text"], "spec-shared#shared（確定）")
        self.assertEqual(req2_rows[0]["cells"][3]["text"], "architecture#second（確定）")

        shared_cells = [
            cell for table in route_tables for row in self.route_rows(table)
            for cell in row["cells"] if "spec-shared#shared" in cell["text"]
        ]
        self.assertGreaterEqual(len(shared_cells), 2)
        self.assertEqual(
            {tuple(cell["links"]) for cell in shared_cells},
            {("specs/spec-shared.md#shared",)},
        )
        self.assertIn("background: #eff6ff", html_text)
        self.assertIn("overflow-wrap: anywhere", html_text)

    def test_requirement_specific_orphan_is_visible_under_its_declared_scope(self):
        self.add_requirement("DIR-REQ-0001", "One", ["DIR-FUNC-0001"])
        self.add_requirement("DIR-REQ-0002", "Two", ["DIR-FUNC-0002"])
        self.add_node("spec-one#one", "spec", ["DIR-REQ-0001"], ["DIR-FUNC-0001"])
        self.add_node("architecture#one", "architecture", ["DIR-REQ-0001"], ["spec-one#one"])
        self.add_node("design-shared#orphan", "design", ["DIR-REQ-0001", "DIR-REQ-0002"], ["architecture#one"])
        html_text = self.generate(allow_errors=True)
        parser = self.parse_html(html_text)
        req2_table = next(
            table for table in parser.tables
            if "trace-table" in table["attrs"].get("class", "")
            and "DIR-REQ-0002" in str(table)
        )
        orphan_rows = [
            row for row in self.route_rows(req2_table)
            if row["group"].get("class") == "group-detached"
            and "design-shared#orphan" in " ".join(cell["text"] for cell in row["cells"])
        ]
        self.assertEqual(len(orphan_rows), 1)
        self.assertIn("上流未接続", orphan_rows[0]["cells"][3]["text"])
        self.assertIn("design-shared#orphan: upstream allocation does not cover DIR-REQ-0002", html_text)

    def test_exception_and_acceptance_are_separate_and_ids_are_escaped(self):
        self.add_requirement("DIR-REQ-0001", "<script>alert(1)</script>", ["DIR-FUNC-0001"], ["DIR-AC-0001"])
        self.add_exception("DIR-REQ-0001")
        self.add_node("DIR-TEST-0001", "verification", ["DIR-REQ-0001"], ["DIR-REQ-0001"])
        html_text = self.generate()
        parser = self.parse_html(html_text)
        route = next(table for table in parser.tables if "trace-table" in table["attrs"].get("class", ""))
        route_rows = self.route_rows(route)
        exception_row = next(row for row in route_rows if row["group"].get("class") == "group-exception")
        self.assertEqual(
            [cell["text"] for cell in exception_row["cells"][2:6]],
            ["非該当（例外）"] * 3 + ["DIR-TEST-0001（確定）"],
        )
        self.assertEqual(exception_row["cells"][1]["text"], "—（例外経路）")
        applicability = next(
            table for table in parser.tables
            if table is not route and "DIR-FUNC-0001" in " ".join(
                cell["text"] for row in table["rows"] for cell in row["cells"]
            )
        )
        self.assertIn("DIR-FUNC-0001", " ".join(cell["text"] for row in applicability["rows"] for cell in row["cells"]))
        acceptance = next(
            table for table in parser.tables
            if "DIR-AC-0001" in " ".join(cell["text"] for row in table["rows"] for cell in row["cells"])
        )
        ac_cells = [cell for row in acceptance["rows"] for cell in row["cells"] if "DIR-AC-0001" in cell["text"]]
        self.assertEqual(len(ac_cells), 1)
        self.assertEqual(ac_cells[0]["links"], ["%E8%A6%81%E4%BB%B6%E5%AE%9A%E7%BE%A9%E6%9B%B8.md#dir-ac-0001"])
        self.assertNotIn("<script>alert(1)</script>", html_text)
        self.assertIn("&lt;script&gt;alert(1)&lt;/script&gt;", html_text)
        markdown = (self.docs / "要件トレーサビリティ一覧.md").read_text(encoding="utf-8")
        self.assertIn('href="要件トレーサビリティ一覧.html"', markdown)
        self.assertIn("<table", markdown)

    def test_exception_optional_function_chain_is_reference_only(self):
        self.add_requirement("DIR-REQ-0001", "Exception", ["DIR-FUNC-0001", "DIR-FUNC-0002"])
        self.add_exception("DIR-REQ-0001")
        self.add_node("spec-optional#optional", "spec", ["DIR-REQ-0001"], ["DIR-FUNC-0001"])
        self.add_node("architecture#optional", "architecture", ["DIR-REQ-0001"], ["spec-optional#optional"])
        self.add_node("DIR-TEST-0001", "verification", ["DIR-REQ-0001"], ["DIR-REQ-0001"])
        parser = self.parse_html(self.generate())
        route = next(table for table in parser.tables if "trace-table" in table["attrs"].get("class", ""))
        rows = self.route_rows(route)
        fulfillment = [row for row in rows if row["group"].get("class") == "group-exception"]
        references = [row for row in rows if row["group"].get("class") == "group-applicability"]
        self.assertEqual(len(fulfillment), 1)
        self.assertEqual(len(references), 1)
        self.assertEqual(fulfillment[0]["cells"][1]["text"], "—（例外経路）")
        self.assertIn("DIR-FUNC-0001", references[0]["cells"][1]["text"])
        self.assertIn("spec-optional#optional", references[0]["cells"][2]["text"])
        self.assertIn("architecture#optional", references[0]["cells"][3]["text"])
        self.assertIn("下流未割当", references[0]["cells"][4]["text"])
        self.assertIn("未到達", references[0]["cells"][5]["text"])
        self.assertIn("詳細設計：下流未割当", references[0]["cells"][7]["text"])
        self.assertNotIn("DIR-FUNC-0002", " ".join(cell["text"] for row in rows for cell in row["cells"]))

    def test_unassigned_and_unknown_refs_are_diagnostic_without_fake_links(self):
        self.add_requirement("DIR-REQ-0001", "One", ["DIR-FUNC-0001"])
        self.add_orphan_function("DIR-FUNC-0999")
        self.add_node("spec-orphan#orphan", "spec", ["DIR-REQ-0001"], ["DIR-FUNC-0001"])
        self.add_node("architecture#bad", "architecture", ["DIR-REQ-0001"], ["missing-node#bad"])
        self.add_node("spec-invalid#invalid", "spec", ["DIR-REQ-0999"], [], pending=["unmapped scope"])
        html_text = self.generate(allow_errors=True)
        self.assertIn("missing-node#bad", html_text)
        self.assertNotIn('href="missing-node#bad"', html_text)
        self.assertIn("有効要件に接続できない", html_text)
        self.assertIn("unknown upstream missing-node#bad", html_text)
        self.assertIn("DIR-FUNC-0999", html_text)
        self.assertIn("未知または無効な要件スコープ：DIR-REQ-0999", html_text)

    def test_html_check_detects_stale_and_missing_html_without_writing(self):
        self.add_requirement("DIR-REQ-0001", "One", ["DIR-FUNC-0001"])
        self.generate()
        html_path = self.docs / "要件トレーサビリティ一覧.html"
        original = html_path.read_text(encoding="utf-8")
        html_path.write_text(original + "stale", encoding="utf-8")
        result = self.run_generator("--check")
        self.assertEqual(result.returncode, 1)
        self.assertIn("要件トレーサビリティ一覧.html is stale", result.stderr)
        self.assertTrue(html_path.read_text(encoding="utf-8").endswith("stale"))
        html_path.unlink()
        result = self.run_generator("--check")
        self.assertEqual(result.returncode, 1)
        self.assertIn("要件トレーサビリティ一覧.html is stale", result.stderr)
        self.assertFalse(html_path.exists())
        self.run_generator()
        md_path = self.docs / "要件トレーサビリティ一覧.md"
        md_path.unlink()
        result = self.run_generator("--check")
        self.assertEqual(result.returncode, 1)
        self.assertIn("要件トレーサビリティ一覧.md is stale", result.stderr)
        self.assertFalse(md_path.exists())

    def test_hierarchy_html_check_detects_stale_and_missing_output_without_writing(self):
        self.add_requirement("DIR-REQ-0001", "One", ["DIR-FUNC-0001"])
        self.generate()
        html_path = self.docs / "要件階層.html"
        original = html_path.read_text(encoding="utf-8")
        html_path.write_text(original + "stale", encoding="utf-8")
        result = self.run_generator("--check")
        self.assertEqual(result.returncode, 1)
        self.assertIn("docs/要件階層.html is stale", result.stderr)
        self.assertTrue(html_path.read_text(encoding="utf-8").endswith("stale"))
        html_path.unlink()
        result = self.run_generator("--check")
        self.assertEqual(result.returncode, 1)
        self.assertIn("docs/要件階層.html is stale", result.stderr)
        self.assertFalse(html_path.exists())

    def test_outputs_are_deterministic_and_check_passes(self):
        self.add_requirement("DIR-REQ-0001", "One", ["DIR-FUNC-0001"])
        self.add_node("spec-one#one", "spec", ["DIR-REQ-0001"], ["DIR-FUNC-0001"])
        first_html = self.generate()
        first_md = (self.docs / "要件トレーサビリティ一覧.md").read_text(encoding="utf-8")
        result = self.run_generator()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.docs / "要件トレーサビリティ一覧.html").read_text(encoding="utf-8"), first_html)
        self.assertEqual((self.docs / "要件トレーサビリティ一覧.md").read_text(encoding="utf-8"), first_md)
        result = self.run_generator("--check")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_dangling_acceptance_anchor_is_diagnostic_and_fatal(self):
        self.add_requirement("DIR-REQ-0001", "One", ["DIR-FUNC-0001"], ["DIR-AC-0999"])
        self.write_docs()
        req_doc = self.docs / "要件定義書.md"
        text = req_doc.read_text(encoding="utf-8")
        req_doc.write_text(text.replace('<a id="dir-ac-0999"></a>**DIR-AC-0999**\n', ""), encoding="utf-8")
        result = self.run_generator()
        self.assertEqual(result.returncode, 1)
        html_text = (self.docs / "要件トレーサビリティ一覧.html").read_text(encoding="utf-8")
        self.assertIn("unknown acceptance-condition anchor DIR-AC-0999", html_text)


if __name__ == "__main__":
    unittest.main()
