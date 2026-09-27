"""Exercise report generation from the staged snapshot in an isolated Git repo."""

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
        for name in ("check_traceability.py", "generate_traceability.py"):
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

    def test_document_version_change_updates_both_from_index_only(self):
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
            self.assertIn("0.1.1", staged)
            self.assertNotIn("9.9.9", staged)
            self.assertEqual(staged, (root / name).read_text(encoding="utf-8"))
        self.assertIn("9.9.9", guide.read_text(encoding="utf-8"))

    def test_staged_html_deletion_regenerates_and_stages_html(self):
        root = self.repository()
        report = root / REPORTS[1]
        expected = report.read_text(encoding="utf-8")
        report.unlink()
        self.command(root, "git", "add", "--", REPORTS[1])

        self.hook(root)

        self.assertEqual(expected, report.read_text(encoding="utf-8"))
        self.assertEqual(expected, self.command(root, "git", "show", ":" + REPORTS[1]))


if __name__ == "__main__":
    unittest.main()
