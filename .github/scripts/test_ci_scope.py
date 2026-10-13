"""Exercise event ranges against real Git histories, including unusual path names."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from ci_scope import metadata_only

SCOPE_SCRIPT = Path(__file__).with_name("ci_scope.py").resolve()


class ScopeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.previous = Path.cwd()
        os.chdir(self.directory.name)
        self.git("init", "-q", "--initial-branch=main")
        self.git("config", "user.name", "CI scope test")
        self.git("config", "user.email", "ci@example.invalid")
        self.base = self.commit("code.rs", "original\n")

    def tearDown(self):
        os.chdir(self.previous)
        self.directory.cleanup()

    def git(self, *args):
        return subprocess.check_output(["git", *args], stderr=subprocess.PIPE).decode().strip()

    def commit(self, path, content):
        target = Path(path)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        self.git("add", "--all")
        self.git("commit", "-qm", "fixture")
        return self.git("rev-parse", "HEAD")

    def check_both_events(self, before, after, expected):
        self.assertEqual(metadata_only("push", {"before": before, "after": after}), expected)
        self.assertEqual(metadata_only("pull_request", {"pull_request": {
            "base": {"sha": before}, "head": {"sha": after},
        }}), expected)

    def test_blame_add_modify_delete(self):
        added = self.commit(".git-blame-ignore-revs", "# comment\n")
        self.check_both_events(self.base, added, True)
        modified = self.commit(".git-blame-ignore-revs", "# another comment\n")
        self.check_both_events(added, modified, True)
        Path(".git-blame-ignore-revs").unlink()
        self.git("add", "--all")
        self.git("commit", "-qm", "delete metadata")
        self.check_both_events(modified, self.git("rev-parse", "HEAD"), True)

    def test_other_paths_and_mixed_changes_run(self):
        metadata = self.commit(".git-blame-ignore-revs", "# comment\n")
        for path in ["code.rs", ".github/workflows/ci.yml", ".github/scripts/ci_scope.py",
                     "notes/design.md", "backend/prompts/worker.md", "nested/.git-blame-ignore-revs"]:
            with self.subTest(path=path):
                self.git("reset", "--hard", metadata)
                head = self.commit(path, "changed\n")
                self.check_both_events(metadata, head, False)
                self.check_both_events(self.base, head, False)

    def test_multiple_commit_push_includes_earlier_code(self):
        code = self.commit("code.rs", "changed\n")
        head = self.commit(".git-blame-ignore-revs", "# comment\n")
        self.check_both_events(code, head, True)
        self.check_both_events(self.base, head, False)

    def test_pr_uses_merge_base_when_base_branch_moves(self):
        head = self.commit(".git-blame-ignore-revs", "# comment\n")
        self.git("checkout", "-q", "-b", "advanced-base", self.base)
        base = self.commit("code.rs", "base advanced\n")
        self.assertTrue(metadata_only("pull_request", {"pull_request": {
            "base": {"sha": base}, "head": {"sha": head},
        }}))
        # The same two endpoints are a different change for push (including a force push).
        self.assertFalse(metadata_only("push", {"before": base, "after": head}))

    def test_rename_into_or_out_of_metadata_runs(self):
        self.git("mv", "code.rs", ".git-blame-ignore-revs")
        self.git("commit", "-qm", "rename into metadata")
        into = self.git("rev-parse", "HEAD")
        self.check_both_events(self.base, into, False)
        self.git("mv", ".git-blame-ignore-revs", "code.rs")
        self.git("commit", "-qm", "rename out of metadata")
        self.check_both_events(into, self.git("rev-parse", "HEAD"), False)

    @unittest.skipIf(os.name == "nt", "Windows paths cannot contain a newline")
    def test_nul_delimited_path_cannot_impersonate_whitelist(self):
        head = self.commit(".git-blame-ignore-revs\ncode.rs", "changed\n")
        self.check_both_events(self.base, head, False)

    def test_cli_output_and_malformed_event_failure(self):
        head = self.commit(".git-blame-ignore-revs", "# comment\n")
        event = Path("event.json")
        event.write_text(json.dumps({"before": self.base, "after": head}), encoding="utf-8")
        output = Path("output.txt")
        env = {**os.environ, "GITHUB_EVENT_NAME": "push", "GITHUB_EVENT_PATH": str(event),
               "GITHUB_OUTPUT": str(output)}
        result = subprocess.run([sys.executable, str(SCOPE_SCRIPT)], env=env, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output.read_text(encoding="utf-8"), "metadata_only=true\n")
        output.unlink()
        event.write_text("invalid JSON", encoding="utf-8")
        result = subprocess.run([sys.executable, str(SCOPE_SCRIPT)], env=env, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())

    def test_unknown_empty_and_missing_ranges_run(self):
        self.check_both_events(self.base, self.base, False)
        for missing in ["", "0" * 40, "1" * 40, "--stat"]:
            self.check_both_events(missing, self.base, False)
        self.assertFalse(metadata_only("workflow_dispatch", {}))
        self.assertFalse(metadata_only("push", {}))
        self.assertFalse(metadata_only("pull_request", {}))
        self.git("checkout", "--orphan", "unrelated")
        other = self.commit(".git-blame-ignore-revs", "# comment\n")
        self.assertFalse(metadata_only("pull_request", {"pull_request": {
            "base": {"sha": self.base}, "head": {"sha": other},
        }}))


if __name__ == "__main__":
    unittest.main()
