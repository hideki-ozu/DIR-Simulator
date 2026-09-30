import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).parents[1] / "scripts" / "check_traceability.py"


class TestTraceabilityChecker(unittest.TestCase):
    def build(
        self,
        *,
        exception=False,
        direct=False,
        shared=False,
        dangling=False,
        pending=False,
    ):
        temp = tempfile.TemporaryDirectory()
        root = Path(temp.name)
        docs = root / "docs"
        docs.mkdir()
        (docs / "specs").mkdir()
        (docs / "design").mkdir()
        (docs / "verification" / "cases").mkdir(parents=True)
        reqs = ["DIR-REQ-0001", "DIR-REQ-0002"] if shared else ["DIR-REQ-0001"]
        (docs / "要件定義書.md").write_text(
            "\n".join(
                [
                    "文書ID：`requirements`",
                    "| 要件ID | 名称 | 要件（条件・対象・期待結果） | 検証先・確認内容 | TBD・注釈 |",
                    "| --- | --- | --- | --- | --- |",
                ]
                + [f'| <a id="{x.lower()}"></a>**{x}** | x | body | test | — |' for x in reqs]
            )
            + (
                "\n```trace-exception\n"
                + json.dumps(
                    {
                        "requirement": "DIR-REQ-0001",
                        "reason": "constraint",
                        "stages": ["requirement", "verification"],
                    }
                )
                + "\n```"
                if exception
                else ""
            ),
            encoding="utf-8",
        )
        hierarchy = [
            {
                "id": requirement,
                "parent": None if index == 0 else reqs[0],
                "position": "上位" if index == 0 else "下位",
                "decomposition": "未分解" if index == 0 else "子要件へ分解",
            }
            for index, requirement in enumerate(reqs)
        ]
        (docs / "要件階層.json").write_text(
            json.dumps({"schema_version": 1, "requirements": hierarchy}, ensure_ascii=False),
            encoding="utf-8",
        )
        rows = "\n".join(
            f"| [{r}]() | x | [DIR-FUNC-0001](#dir-func-0001) |" for r in reqs
        )
        (docs / "機能仕様書.md").write_text(
            '文書ID：`functions`\n<a id="dir-func-0001"></a>**DIR-FUNC-0001**\n### 11.2. map\n| a | b | c |\n|---|---|---|\n'
            + rows,
            encoding="utf-8",
        )

        def doc(
            path,
            docid,
            anchor,
            stage,
            upstream,
            reqscope=None,
            state="confirmed",
            pend=[],
        ):
            data = {
                "id": (
                    "DIR-TEST-0001" if stage == "verification" else f"{docid}#{anchor}"
                ),
                "stage": stage,
                "requirements": reqscope or reqs,
                "upstream": upstream,
                "state": state,
                "pending": pend,
            }
            path.write_text(
                f'文書ID：`{docid}`\n<a id="{anchor}"></a>\n```trace\n{json.dumps(data)}\n```',
                encoding="utf-8",
            )

        if exception:
            doc(
                docs / "verification" / "cases" / "v.md",
                "v",
                "dir-test-0001",
                "verification",
                ["DIR-REQ-0001"],
            )
        else:
            doc(
                docs / "specs" / "s.md",
                "s",
                "s",
                "spec",
                ["DIR-FUNC-0001"],
                state="draft" if pending else "confirmed",
                pend=["waiting"] if pending else [],
            )
            doc(docs / "アーキテクチャ設計書.md", "a", "a", "architecture", ["s#s"])
            if not dangling:
                doc(docs / "design" / "d.md", "d", "d", "design", ["a#a"])
                doc(
                    docs / "verification" / "cases" / "v.md",
                    "v",
                    "dir-test-0001",
                    "verification",
                    ["d#d"],
                )
        return temp, root

    def check(self, root, *args):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(root), *args],
            text=True,
            capture_output=True,
        )

    def test_complete_chain_is_structurally_and_strictly_valid(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        self.assertEqual(self.check(root).returncode, 0)
        self.assertEqual(self.check(root, "--strict").returncode, 0)

    def test_document_id_examples_do_not_create_definitions(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        guide = root / "docs/規約.md"
        guide.write_text(
            "文書ID：`guide`\n- 冒頭に `文書ID：` を記載し、`example` 等を使う。\n```json\n文書ID：`example`\n```\n",
            encoding="utf-8",
        )
        result = self.check(root, "--strict")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_only_anchored_requirement_table_rows_are_canonical(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        path = root / "docs/要件定義書.md"
        path.write_text(
            path.read_text(encoding="utf-8")
            + "\n\n| `DIR-REQ-0099` | 廃止 | — | — | — |\n"
            + '<a id="dir-req-0098"></a>**DIR-REQ-0098**\n',
            encoding="utf-8",
        )
        result = self.check(root, "--strict")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("1 requirements", result.stdout)

    def test_requirement_table_anchor_must_match_its_id(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        path = root / "docs/要件定義書.md"
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                'id="dir-req-0001"', 'id="dir-req-0002"'
            ),
            encoding="utf-8",
        )
        result = self.check(root)
        self.assertEqual(result.returncode, 1)
        self.assertIn("requirement anchor mismatch", result.stderr)

    def test_three_and_five_digit_ids_do_not_match_four_digit_ids(self):
        for malformed in ("DIR-REQ-001", "DIR-REQ-00001"):
            with self.subTest(malformed=malformed):
                temp, root = self.build()
                self.addCleanup(temp.cleanup)
                path = root / "docs/機能仕様書.md"
                path.write_text(
                    path.read_text(encoding="utf-8").replace(
                        "[DIR-REQ-0001]()", f"[{malformed}]()"
                    ),
                    encoding="utf-8",
                )
                result = self.check(root)
                self.assertEqual(result.returncode, 1)
                self.assertIn("has 0 mapping rows", result.stderr)

    def test_malformed_requirement_table_id_is_reported(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        path = root / "docs/要件定義書.md"
        path.write_text(
            path.read_text(encoding="utf-8").replace(
                "**DIR-REQ-0001**", "**DIR-REQ-001**"
            ),
            encoding="utf-8",
        )
        result = self.check(root)
        self.assertEqual(result.returncode, 1)
        self.assertIn("invalid requirement ID cell", result.stderr)

    def test_dangling_branch_fails_strict_even_with_no_structural_error(self):
        temp, root = self.build(dangling=True)
        self.addCleanup(temp.cleanup)
        self.assertEqual(self.check(root).returncode, 0)
        result = self.check(root, "--strict")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no design continuation", result.stdout)

    def test_pending_is_integrity_but_not_complete_and_confirmed_pending_is_invalid(
        self,
    ):
        temp, root = self.build(pending=True)
        self.addCleanup(temp.cleanup)
        self.assertEqual(self.check(root).returncode, 0)
        self.assertEqual(self.check(root, "--strict").returncode, 1)
        path = root / "docs/specs/s.md"
        path.write_text(
            path.read_text().replace('"draft"', '"confirmed"'), encoding="utf-8"
        )
        self.assertEqual(self.check(root).returncode, 1)

    def test_exception_requires_direct_verification(self):
        temp, root = self.build(exception=True)
        self.addCleanup(temp.cleanup)
        self.assertEqual(self.check(root, "--strict").returncode, 0)
        p = root / "docs/verification/cases/v.md"
        p.unlink()
        self.assertEqual(self.check(root, "--strict").returncode, 1)

    def test_illegal_shortcut_unknown_ref_duplicate_and_anchor_errors(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        p = root / "docs/specs/s.md"
        text = p.read_text().replace("DIR-FUNC-0001", "DIR-REQ-0001")
        p.write_text(text, encoding="utf-8")
        self.assertEqual(self.check(root).returncode, 1)
        temp2, root2 = self.build()
        self.addCleanup(temp2.cleanup)
        p2 = root2 / "docs/specs/s.md"
        p2.write_text(
            p2.read_text()
            .replace("s#s", "s#missing")
            .replace("DIR-FUNC-0001", "DIR-FUNC-0999"),
            encoding="utf-8",
        )
        self.assertEqual(self.check(root2).returncode, 1)

    def test_shared_function_scope_and_impact_are_requirement_filtered(self):
        temp, root = self.build(shared=True)
        self.addCleanup(temp.cleanup)
        # Remove REQ-002 from every node: REQ-001's route cannot satisfy it.
        for p in (root / "docs").rglob("*.md"):
            if p.name != "要件定義書.md" and p.name != "機能仕様書.md":
                p.write_text(
                    p.read_text().replace(', "DIR-REQ-0002"', ""), encoding="utf-8"
                )
        result = self.check(root, "--strict", "--requirement", "DIR-REQ-0002")
        self.assertEqual(result.returncode, 1)
        self.assertIn("has no spec allocation", result.stdout)
        impact = self.check(root, "--impact", "DIR-REQ-0001")
        self.assertEqual(impact.returncode, 0)
        self.assertIn("downstream affected", impact.stdout)

    def test_default_reports_incomplete_but_does_not_fail(self):
        temp, root = self.build(dangling=True)
        self.addCleanup(temp.cleanup)
        result = self.check(root)
        self.assertEqual(result.returncode, 0)
        self.assertIn("incomplete:", result.stdout)

    def test_missing_requirement_scope_coverage_is_structural_error(self):
        temp, root = self.build(shared=True)
        self.addCleanup(temp.cleanup)
        # The architecture node only receives a spec allocation scoped to REQ-001.
        spec = root / "docs/specs/s.md"
        spec.write_text(
            spec.read_text().replace(', "DIR-REQ-0002"', ""), encoding="utf-8"
        )
        result = self.check(root)
        self.assertEqual(result.returncode, 1)
        self.assertIn("does not cover DIR-REQ-0002", result.stderr)

    def test_duplicate_document_id_anchor_and_json_key_fail_cleanly(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        extra = root / "docs/design/extra.md"
        extra.write_text(
            '文書ID：`d`\n<a id="x"></a>\n<a id="x"></a>\n```trace\n{"id":"d#x","id":"d#x","stage":"design","requirements":["DIR-REQ-0001"],"upstream":[],"state":"draft","pending":["waiting"]}\n```',
            encoding="utf-8",
        )
        result = self.check(root)
        self.assertEqual(result.returncode, 1)
        self.assertIn("duplicate document ID", result.stderr)
        self.assertIn("duplicate explicit anchor", result.stderr)
        self.assertIn("duplicate JSON key", result.stderr)

    def test_empty_upstream_without_pending_is_structural_error(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        path = root / "docs/specs/s.md"
        path.write_text(
            path.read_text().replace('["DIR-FUNC-0001"]', "[]"), encoding="utf-8"
        )
        result = self.check(root)
        self.assertEqual(result.returncode, 1)
        self.assertIn("upstream=[] requires", result.stderr)

    def test_function_impact_reports_missing_spec_allocation(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        for name in (
            "specs/s.md",
            "アーキテクチャ設計書.md",
            "design/d.md",
            "verification/cases/v.md",
        ):
            (root / "docs" / name).unlink()
        result = self.check(root, "--impact", "DIR-FUNC-0001")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("DIR-REQ-0001:DIR-FUNC-0001 missing spec allocation", result.stdout)

    def edit_node(self, root, name, **changes):
        path = root / "docs" / name
        text = path.read_text(encoding="utf-8")
        match = re.search(r"```trace\n(.*?)\n```", text, re.S)
        data = json.loads(match[1])
        data.update(changes)
        path.write_text(
            text[: match.start(1)] + json.dumps(data) + text[match.end(1) :],
            encoding="utf-8",
        )

    def test_complete_route_does_not_hide_an_unfinished_branch(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        source = root / "docs/design/d.md"
        branch = root / "docs/design/branch.md"
        branch.write_text(
            source.read_text().replace("`d`", "`branch`").replace("d#d", "branch#d"),
            encoding="utf-8",
        )
        result = self.check(root, "--strict")
        self.assertEqual(result.returncode, 1)
        self.assertIn("branch#d has no verification continuation", result.stdout)

    def test_complete_route_does_not_hide_a_detached_pending_case(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        source = root / "docs/verification/cases/v.md"
        extra = root / "docs/verification/cases/extra.md"
        extra.write_text(
            source.read_text()
            .replace("`v`", "`extra`")
            .replace("TEST-0001", "TEST-0002")
            .replace("test-0001", "test-0002"),
            encoding="utf-8",
        )
        self.edit_node(
            root,
            "verification/cases/extra.md",
            upstream=[],
            state="draft",
            pending=["unallocated"],
        )
        result = self.check(root, "--strict")
        self.assertEqual(result.returncode, 1)
        self.assertIn("DIR-TEST-0002 has no upstream allocation", result.stdout)

    def test_draft_structure_can_pass_but_pending_exception_cannot(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        self.edit_node(root, "specs/s.md", state="draft")
        self.assertEqual(self.check(root, "--strict").returncode, 0)
        temp, root = self.build(exception=True)
        self.addCleanup(temp.cleanup)
        result = self.check(root, "--strict", "--impact", "DIR-REQ-0001")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("pending gaps: (none)", result.stdout)
        self.edit_node(
            root,
            "verification/cases/v.md",
            state="draft",
            pending=["review specification missing"],
        )
        result = self.check(root, "--strict")
        self.assertEqual(result.returncode, 1)
        self.assertIn("DIR-TEST-0001 has pending work", result.stdout)

    def test_impact_does_not_cross_requirement_scopes_at_a_merge(self):
        temp, root = self.build(shared=True)
        self.addCleanup(temp.cleanup)
        functions = root / "docs/機能仕様書.md"
        functions.write_text(
            functions.read_text()
            .replace(
                "### 11.2.", '<a id="dir-func-0002"></a>**DIR-FUNC-0002**\n### 11.2.'
            )
            .replace(
                "| [DIR-REQ-0002]() | x | [DIR-FUNC-0001](#dir-func-0001) |",
                "| [DIR-REQ-0002]() | x | [DIR-FUNC-0002](#dir-func-0002) |",
            ),
            encoding="utf-8",
        )
        # Shared spec and architecture carry both scopes; design and test only REQ-001.
        self.edit_node(root, "specs/s.md", upstream=["DIR-FUNC-0001", "DIR-FUNC-0002"])
        self.edit_node(root, "design/d.md", requirements=["DIR-REQ-0001"])
        self.edit_node(root, "verification/cases/v.md", requirements=["DIR-REQ-0001"])
        result = self.check(root, "--impact", "d#d")
        self.assertEqual(result.returncode, 0, result.stderr)
        upstream = next(
            line
            for line in result.stdout.splitlines()
            if line.startswith("upstream roots:")
        )
        self.assertIn("DIR-FUNC-0001", upstream)
        self.assertNotIn("DIR-FUNC-0002", upstream)
        self.assertNotIn("DIR-REQ-0002", upstream)
        result = self.check(root, "--impact", "DIR-REQ-0002")
        downstream = next(
            line
            for line in result.stdout.splitlines()
            if line.startswith("downstream affected:")
        )
        self.assertNotIn("d#d", downstream)
        self.assertNotIn("DIR-TEST-0001", downstream)

    def test_noncanonical_ids_invalid_types_and_cycles_fail_without_tracebacks(self):
        for name, changes in [
            ("verification/cases/v.md", {"id": "dir-test-0001"}),
            ("specs/s.md", {"id": "s#S"}),
            ("specs/s.md", {"stage": {"invalid": True}}),
            ("specs/s.md", {"requirements": [None]}),
            ("specs/s.md", {"upstream": ["d#d"]}),
        ]:
            with self.subTest(changes=changes):
                temp, root = self.build()
                self.addCleanup(temp.cleanup)
                self.edit_node(root, name, **changes)
                result = self.check(root, "--strict", "--impact", "DIR-FUNC-0001")
                self.assertEqual(result.returncode, 1)
                self.assertIn("error:", result.stderr)
                self.assertNotIn("Traceback", result.stderr)

    def test_function_ranges_only_use_the_function_column(self):
        temp, root = self.build()
        self.addCleanup(temp.cleanup)
        path = root / "docs/機能仕様書.md"
        text = path.read_text().replace(
            "### 11.2.",
            '<a id="dir-func-0002"></a>**DIR-FUNC-0002**\n<a id="dir-func-0003"></a>**DIR-FUNC-0003**\n### 11.2.',
        )
        text = text.replace(
            "| x | [DIR-FUNC-0001](#dir-func-0001) |",
            "| DIR-FUNC-0999 | [DIR-FUNC-0001](#dir-func-0001)〜[DIR-FUNC-0003](#dir-func-0003) | DIR-FUNC-0998 |",
        )
        path.write_text(text, encoding="utf-8")
        result = self.check(root, "--strict")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stderr, "")
        self.assertIn("DIR-FUNC-0002 has no spec allocation", result.stdout)
        self.assertIn("DIR-FUNC-0003 has no spec allocation", result.stdout)

    def test_duplicate_sources_missing_document_ids_and_empty_sources_are_errors(self):
        for name, change in [
            ("要件定義書.md", lambda t: t + '\n| <a id="dir-req-0001"></a>**DIR-REQ-0001** | duplicate | body | test | — |'),
            ("機能仕様書.md", lambda t: t + "\n| [DIR-REQ-0001]() | x | DIR-FUNC-0001 |"),
            ("アーキテクチャ設計書.md", lambda t: t + "\n文書ID：`another`"),
            ("アーキテクチャ設計書.md", lambda t: t.replace("文書ID：`a`", "")),
            ("要件定義書.md", lambda t: "文書ID：`requirements`"),
            ("機能仕様書.md", lambda t: "文書ID：`functions`\n### 11.2. map"),
        ]:
            with self.subTest(name=name):
                temp, root = self.build()
                self.addCleanup(temp.cleanup)
                path = root / "docs" / name
                path.write_text(change(path.read_text()), encoding="utf-8")
                result = self.check(root)
                self.assertEqual(result.returncode, 1)
                self.assertIn("error:", result.stderr)


if __name__ == "__main__":
    unittest.main()
