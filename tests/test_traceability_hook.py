"""Exercise report generation from the staged snapshot in an isolated Git repo."""

import json
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

import test_check_traceability as checker_tests


REPOSITORY = Path(__file__).resolve().parents[1]
REPORTS = (
    "docs/要件トレーサビリティ一覧.md",
    "docs/要件トレーサビリティ一覧.html",
    "docs/要件階層.html",
)


class TestTraceabilityHook(unittest.TestCase):
    def command(self, root, *args):
        result = subprocess.run(args, cwd=root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def repository(self):
        temporary, root = checker_tests.TestTraceabilityChecker().build()
        self.addCleanup(temporary.cleanup)
        (root / "scripts").mkdir()
        for name in (
            "check_traceability.py",
            "generate_traceability.py",
            "requirement_hierarchy.py",
        ):
            shutil.copy2(REPOSITORY / "scripts" / name, root / "scripts" / name)
        (root / "docs/規約.md").write_text(
            "# 規約\n文書バージョン：`0.1.0`\n文書ID：`guide`\n", encoding="utf-8"
        )
        self.command(root, sys.executable, "scripts/generate_traceability.py")
        self.command(root, "git", "init", "--quiet")
        self.command(root, "git", "add", ".")
        self.command(
            root, "git", "-c", "user.name=Traceability Test",
            "-c", "user.email=traceability@example.invalid",
            "commit", "--quiet", "-m", "fixture",
        )
        return root

    def hook(self, root):
        self.command(root, sys.executable, str(REPOSITORY / ".githooks/pre-commit"))

    def test_document_version_change_updates_all_reports_from_index(self):
        root = self.repository()
        guide = root / "docs/規約.md"
        guide.write_text(
            "# 規約\n文書バージョン：`0.1.1`\n文書ID：`guide`\n", encoding="utf-8"
        )
        self.command(root, "git", "add", "docs/規約.md")
        guide.write_text(
            "# 規約\n文書バージョン：`9.9.9`\n文書ID：`guide`\n", encoding="utf-8"
        )

        self.hook(root)

        for name in REPORTS:
            staged = self.command(root, "git", "show", ":" + name)
            self.assertNotIn("9.9.9", staged)
            self.assertEqual(staged, (root / name).read_text(encoding="utf-8"))
        for name in REPORTS[:2]:
            self.assertIn("0.1.1", self.command(root, "git", "show", ":" + name))
        self.assertIn("9.9.9", guide.read_text(encoding="utf-8"))

    def test_staged_hierarchy_json_change_uses_index_only(self):
        root = self.repository()
        source = root / "docs/要件階層.json"
        staged_data = json.loads(source.read_text(encoding="utf-8"))
        staged_data["requirements"][0]["decomposition"] = "staged & <value>"
        source.write_text(
            json.dumps(staged_data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        self.command(root, "git", "add", "docs/要件階層.json")
        staged_text = source.read_text(encoding="utf-8")
        unstaged_data = json.loads(staged_text)
        unstaged_data["requirements"][0]["decomposition"] = "unstaged value"
        source.write_text(
            json.dumps(unstaged_data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )

        self.hook(root)

        hierarchy = REPORTS[2]
        staged_html = self.command(root, "git", "show", ":" + hierarchy)
        self.assertIn("staged &amp; &lt;value&gt;", staged_html)
        self.assertNotIn("unstaged value", staged_html)
        self.assertEqual(staged_html, (root / hierarchy).read_text(encoding="utf-8"))
        self.assertIn("unstaged value", source.read_text(encoding="utf-8"))

    def test_staged_html_deletions_regenerate_and_stage_each_html_report(self):
        for name in REPORTS[1:]:
            with self.subTest(name=name):
                root = self.repository()
                report = root / name
                expected = report.read_text(encoding="utf-8")
                report.unlink()
                self.command(root, "git", "add", "--", name)

                self.hook(root)

                self.assertEqual(expected, report.read_text(encoding="utf-8"))
                self.assertEqual(expected, self.command(root, "git", "show", ":" + name))

    def test_staged_hierarchy_module_change_regenerates_hierarchy_html(self):
        root = self.repository()
        module = root / "scripts/requirement_hierarchy.py"
        text = module.read_text(encoding="utf-8")
        self.assertIn("分解状態を示します", text)
        module.write_text(text.replace("分解状態を示します", "分解メモを示します"), encoding="utf-8")
        self.command(root, "git", "add", "scripts/requirement_hierarchy.py")

        self.hook(root)

        hierarchy = REPORTS[2]
        staged_html = self.command(root, "git", "show", ":" + hierarchy)
        self.assertIn("分解メモを示します", staged_html)
        self.assertEqual(staged_html, (root / hierarchy).read_text(encoding="utf-8"))

    def test_invalid_staged_hierarchy_leaves_worktree_and_index_reports_unchanged(self):
        root = self.repository()
        source = root / "docs/要件階層.json"
        valid = source.read_text(encoding="utf-8")
        data = json.loads(valid)
        data["requirements"][0]["parent"] = "DIR-REQ-9999"
        source.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        self.command(root, "git", "add", "docs/要件階層.json")
        source.write_text(valid, encoding="utf-8")
        before = {name: (root / name).read_bytes() for name in REPORTS}
        indexed = {name: self.command(root, "git", "show", ":" + name) for name in REPORTS}

        result = subprocess.run(
            [sys.executable, str(REPOSITORY / ".githooks/pre-commit")],
            cwd=root,
            capture_output=True,
            text=True,
        )

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("unknown parent DIR-REQ-9999", result.stderr)
        for name in REPORTS:
            self.assertEqual((root / name).read_bytes(), before[name])
            self.assertEqual(self.command(root, "git", "show", ":" + name), indexed[name])
        self.assertEqual(source.read_text(encoding="utf-8"), valid)


if __name__ == "__main__":
    unittest.main()
