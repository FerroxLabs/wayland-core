#!/usr/bin/env python3
"""Real Git fixtures prove metadata cannot substitute product or gate code."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "overlay", ROOT / ".github/scripts/release-metadata-overlay.py")
OVERLAY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OVERLAY)

# These fixture checkers verify the protocol the wrapper must preserve: their
# __file__ root is a tag checkout, both corrected ledgers are visible, and the
# tag product remains unchanged. A failing checker must block any passed receipt.
CHECKER = '''import os, sys
from pathlib import Path
root = Path(__file__).resolve().parents[1]
assert (root / "product.rs").read_text() == "immutable product"
assert (root / ".planning/ledger/wayland-1116.md").read_text() == "corrected"
assert (root / ".planning/ledger/wayland-core-443.md").read_text() == "corrected"
if os.environ.get("FAIL_CHECKER") == Path(__file__).name and "--self-test" not in sys.argv:
    sys.exit(7)
'''


class Metadata(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        self.receipt = Path(self.temp.name) / "receipt.json"
        self.git("init", "-q")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        (self.root / ".planning/ledger").mkdir(parents=True)
        (self.root / "scripts").mkdir()
        (self.root / "product.rs").write_text("immutable product")
        (self.root / ".planning/ledger/wayland-core-443.md").write_text("original")
        for checker in OVERLAY.CHECKERS:
            (self.root / "scripts" / checker).write_text(CHECKER)
        self.source = self.save()
        self.git("tag", "v1.2.3")
        for path in OVERLAY.ALLOWED:
            (self.root / path).write_text("corrected")
        self.metadata = self.save()
        self.git("checkout", "--detach", self.source)

    def git(self, *args):
        return OVERLAY.git(self.root, *args).decode().strip()

    def save(self):
        self.git("add", ".")
        self.git("commit", "-qm", "fixture")
        return self.git("rev-parse", "HEAD")

    def amend_metadata(self, path, contents):
        self.git("checkout", "--detach", self.metadata)
        if contents is None:
            (self.root / path).unlink()
        else:
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_text(contents)
        self.git("add", "-A")
        self.git("commit", "--amend", "--no-edit", "-q")
        self.metadata = self.git("rev-parse", "HEAD")
        self.git("checkout", "--detach", self.source)

    def run_overlay(self, metadata=None):
        return OVERLAY.run(self.root, "v1.2.3", self.source,
                           metadata or self.metadata, self.receipt)

    def assert_blocked(self):
        self.receipt.write_text('{"status":"passed"}')
        with self.assertRaises((ValueError, subprocess.CalledProcessError)):
            self.run_overlay()
        self.assertFalse(self.receipt.exists())
        self.assertEqual(self.git("status", "--porcelain"), "")

    def test_committed_overlay_runs_all_tag_gates_without_changing_source(self):
        report = self.run_overlay()
        self.assertEqual(report["status"], "passed")
        self.assertEqual(len(report["checks"]), 4)
        self.assertEqual(report["metadata_sha"], self.metadata)
        self.assertEqual(report["source_sha"], self.source)
        self.assertEqual(set(report["ledger_sha256"]), OVERLAY.ALLOWED)
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.git("rev-parse", "HEAD"), self.source)
        self.assertEqual(self.git("worktree", "list", "--porcelain").count("worktree "), 1)

    def test_wrong_or_abbreviated_sha(self):
        for sha in ("deadbeef", "g" * 40, "0" * 40, "main", "--help"):
            with self.subTest(sha=sha), self.assertRaises((ValueError, subprocess.CalledProcessError)):
                self.run_overlay(sha)

    def test_foreign_path(self):
        self.amend_metadata(".planning/ledger/wayland-9999.md", "foreign")
        self.assert_blocked()

    def test_product_substitution(self):
        self.amend_metadata("product.rs", "different product")
        self.assert_blocked()

    def test_checker_substitution(self):
        self.amend_metadata("scripts/check-release-readiness.py", "raise SystemExit(0)")
        self.assert_blocked()

    def test_missing_approved_file(self):
        self.amend_metadata(".planning/ledger/wayland-core-443.md", None)
        self.assert_blocked()

    def test_symlink_is_not_ledger_metadata(self):
        self.git("checkout", "--detach", self.metadata)
        path = self.root / ".planning/ledger/wayland-core-443.md"
        path.unlink()
        path.symlink_to("wayland-1116.md")
        self.git("add", "-A")
        self.git("commit", "--amend", "--no-edit", "-q")
        self.metadata = self.git("rev-parse", "HEAD")
        self.git("checkout", "--detach", self.source)
        self.assert_blocked()

    def test_stale_or_moved_tag(self):
        self.git("tag", "-f", "v1.2.3", self.metadata)
        self.assert_blocked()

    def test_metadata_based_on_other_source(self):
        self.git("checkout", "--detach", self.metadata)
        self.git("commit", "--allow-empty", "-qm", "extra parent")
        self.metadata = self.git("rev-parse", "HEAD")
        self.git("checkout", "--detach", self.source)
        self.assert_blocked()

    def test_each_failed_live_checker_blocks_and_cleans_up(self):
        for checker in OVERLAY.CHECKERS:
            with self.subTest(checker=checker), patch.dict(os.environ, {"FAIL_CHECKER": checker}):
                with self.assertRaisesRegex(RuntimeError, "tag checker failed"):
                    self.run_overlay()
                report = json.loads(self.receipt.read_text())
                self.assertEqual(report["status"], "blocked")
                self.assertEqual(report["checks"][-1]["exit_code"], 7)
                self.assertEqual(self.git("status", "--porcelain"), "")
                self.assertEqual(self.git("worktree", "list", "--porcelain").count("worktree "), 1)


def audit_workflow(text):
    errors = []
    if '${GITHUB_SHA}' in text or '${{ github.sha }}' in text:
        errors.append("workflow identity used as product source")
    if '--source-commit "$(git -C source rev-parse HEAD)"' not in text:
        errors.append("manifest must bind checked-out tag source")
    refs = [line.strip() for line in text.splitlines() if line.strip().startswith("ref:")]
    if not refs or any(line != "ref: refs/tags/${{ inputs.tag_name || github.ref_name }}" for line in refs):
        errors.append("product checkout no longer pinned to tag")
    for name in ("Criteria ledger covers both trackers", "No in-scope defect still owes core work"):
        if name + "\n        if: inputs.release_metadata_ref == ''" not in text:
            errors.append("default live gate missing")
    for required in ('WORKFLOW_CONTROL_SHA: ${{ github.workflow_sha }}',
                     'git fetch --no-tags origin "$RELEASE_METADATA_SHA" "$WORKFLOW_CONTROL_SHA"',
                     '--metadata-sha "$RELEASE_METADATA_SHA"',
                     'name: release-metadata-receipt', 'if-no-files-found: error'):
        if required not in text:
            errors.append("missing metadata authentication or receipt: " + required)
    return errors


class Workflow(unittest.TestCase):
    def test_product_and_control_identities_are_separate(self):
        text = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertEqual(audit_workflow(text), [])
        self.assertTrue(audit_workflow(text.replace(
            '--source-commit "$(git -C source rev-parse HEAD)"', '--source-commit "${GITHUB_SHA}"')))
        self.assertTrue(audit_workflow(text.replace(
            'ref: refs/tags/${{ inputs.tag_name || github.ref_name }}', 'ref: ${{ github.sha }}', 1)))
        self.assertTrue(audit_workflow(text.replace('--metadata-sha "$RELEASE_METADATA_SHA"', '')))


if __name__ == "__main__":
    unittest.main()
