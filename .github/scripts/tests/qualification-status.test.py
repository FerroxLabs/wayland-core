#!/usr/bin/env python3
"""Real Git fixture controls for conservative planning and immutable reuse."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("qualification", ROOT / "scripts/qualification-status.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class Qualification(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        self.run_git("init", "-q")
        self.run_git("config", "user.name", "Qualification Fixture")
        self.run_git("config", "user.email", "fixture@example.invalid")
        for name, text in {"Cargo.toml": "fixture manifest", "crates/pkg/src/lib.rs": "production",
                           "crates/pkg/tests/case.rs": "test baseline"}.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.base = self.commit()
        (self.repo / "crates/pkg/tests/case.rs").write_text("corrected fixture")
        self.candidate = self.commit()
        self.manifest = {"schema": 1, "candidate_sha": self.candidate, "pr_head_sha": self.candidate,
                         "release_tag": "v1.0.0", "checks": [self.check("build", "build", "build"),
                                                                self.check("test", "affected", "test")]}
        self.manifest["checks"][0]["inputs"] = ["Cargo.toml", "crates/pkg/src/lib.rs"]
        self.manifest["checks"][1]["inputs"] = ["Cargo.toml", "crates/pkg/src/lib.rs", "crates/pkg/tests/case.rs"]
        self.manifest["checks"][1]["cargo_targets"] = ["pkg::test::case"]
        self.metadata = {"source_sha": self.candidate, "cargo": {
            "workspace_root": str(self.repo), "workspace_members": ["pkg-id"], "packages": [{
                "id": "pkg-id", "name": "pkg", "targets": [{"kind": ["test"], "name": "case",
                    "src_path": str(self.repo / "crates/pkg/tests/case.rs")}]}]}}
        self.state = {"checkout_sha": self.candidate, "pr_head_sha": self.candidate, "worktree_clean": True}
        self.receipts = [self.receipt(self.manifest["checks"][0], self.base),
                         self.receipt(self.manifest["checks"][1], self.candidate)]

    def run_git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], stderr=subprocess.DEVNULL).decode().strip()

    def commit(self):
        self.run_git("add", ".")
        self.run_git("commit", "-qm", "fixture snapshot")
        return self.run_git("rev-parse", "HEAD")

    def check(self, key, stage, kind):
        return {"id": key, "stage": stage, "kind": kind, "owner": "core",
                "command": ["cargo", "build"] if kind == "build" else ["cargo", "nextest", "run", "-p", "pkg", "--test", "case"],
                "toolchain": {"rustc": "fixture-rustc"}, "features": [], "target": "fixture-linux-target",
                "runtime_contract": {"platform": "linux", "fixture": "version-1"},
                "inputs_complete": True, "inputs": ["Cargo.toml"]}

    def receipt(self, check, source):
        row = {key: copy.deepcopy(check[key]) for key in ("id", "command", "toolchain", "features", "target", "runtime_contract")}
        row.update(source_sha=source, complete=True, status="passed", exit_code=0, attempts=1, flaky=False,
                   input_sha256=gate.input_digests(self.repo, source, check["inputs"]))
        if check["kind"] == "test":
            row.update(selected=1, passed=1, failed=0, skipped=0, retries=0)
        return row

    def report(self, receipts=None, state=None, manifest=None):
        return gate.readiness(self.repo, manifest or self.manifest, state or self.state,
                              self.receipts if receipts is None else receipts)

    def test_fixture_only_plan_has_real_cargo_selectors_and_keeps_required_checks(self):
        result = gate.plan(self.repo, self.base, self.manifest, self.metadata)
        self.assertEqual(result["disposition"], "affected")
        self.assertEqual(result["next_checks"], ["test"])
        self.assertEqual(result["required_checks"], ["build", "test"])
        self.assertEqual(result["affected_targets"]["pkg::test::case"]["cargo_selectors"], ["-p", "pkg", "--test", "case"])
        self.assertEqual(result["commands"][0]["argv"], self.manifest["checks"][1]["command"])
        self.assertTrue(result["requires_verified_baseline"])

    def test_ambiguous_fixture_and_stale_metadata_cannot_get_affected_plan(self):
        metadata = copy.deepcopy(self.metadata)
        metadata["cargo"]["packages"][0]["targets"].append({"kind": ["test"], "name": "second",
            "src_path": str(self.repo / "crates/pkg/tests/case.rs")})
        self.assertEqual(gate.plan(self.repo, self.base, self.manifest, metadata)["disposition"], "full")
        metadata["source_sha"] = self.base
        with self.assertRaises(gate.Refused):
            gate.plan(self.repo, self.base, self.manifest, metadata)

    def test_shared_helpers_production_manifests_and_unknowns_require_full(self):
        for path in ["crates/pkg/tests/support/helper.rs", "crates/pkg/src/lib.rs", "Cargo.toml", ".config/nextest.toml", "unknown.txt"]:
            with self.subTest(path=path):
                self.run_git("reset", "--hard", self.candidate)
                target = self.repo / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("changed " + path)
                candidate = self.commit()
                manifest = {**self.manifest, "candidate_sha": candidate}
                metadata = {**self.metadata, "source_sha": candidate}
                result = gate.plan(self.repo, self.candidate, manifest, metadata)
                self.assertEqual(result["disposition"], "full")
                self.assertEqual(result["next_checks"], ["build", "test"])

    def test_reuse_preserves_original_source_and_phases_are_distinct(self):
        original = copy.deepcopy(self.receipts)
        result = self.report()
        self.assertTrue(result["ready"])
        self.assertFalse(result["complete"])
        self.assertTrue(result["phases"]["built"]["passed"])
        self.assertTrue(result["phases"]["verified"]["passed"])
        self.assertFalse(result["phases"]["merged"]["passed"])
        self.assertFalse(result["phases"]["published"]["passed"])
        self.assertEqual(result["checks"][0]["mode"], "reused")
        self.assertEqual(result["checks"][0]["receipt_source_sha"], self.base)
        self.assertEqual(self.receipts, original)
        self.assertIn(self.base, gate.markdown(result))

    def test_command_toolchain_feature_target_and_runtime_mutations_refuse_reuse(self):
        for field, value in [("command", ["cargo", "check"]), ("toolchain", {"rustc": "other"}),
                             ("features", ["different"]), ("target", "windows"),
                             ("runtime_contract", {"platform": "other"})]:
            with self.subTest(field=field):
                rows = copy.deepcopy(self.receipts)
                rows[0][field] = value
                self.assertFalse(self.report(rows)["ready"])

    def test_input_mutation_missing_closure_and_forged_source_digests_refuse(self):
        rows = copy.deepcopy(self.receipts)
        rows[0]["input_sha256"].pop("Cargo.toml")
        self.assertFalse(self.report(rows)["ready"])
        manifest = copy.deepcopy(self.manifest)
        manifest["checks"][0]["inputs_complete"] = False
        self.assertFalse(self.report(manifest=manifest)["ready"])
        (self.repo / "crates/pkg/src/lib.rs").write_text("new production behavior")
        changed = self.commit()
        manifest = {**self.manifest, "candidate_sha": changed, "pr_head_sha": changed}
        state = {**self.state, "checkout_sha": changed, "pr_head_sha": changed}
        self.assertFalse(self.report(state=state, manifest=manifest)["ready"])
        # Even a supplied digest rewritten to match the candidate cannot change
        # the bytes at the original receipt's immutable Git source.
        rows = copy.deepcopy(self.receipts)
        rows[0]["input_sha256"] = gate.input_digests(self.repo, changed, manifest["checks"][0]["inputs"])
        result = self.report(rows, state, manifest)
        self.assertEqual(result["checks"][0]["status"], "blocked")
        self.assertIn("original source", result["checks"][0]["reason"])

    def test_missing_failed_flaky_skipped_and_empty_receipts_never_verify(self):
        self.assertFalse(self.report([])["ready"])
        for field, value in [("status", "failed"), ("status", "pending"), ("status", "skipped"),
                             ("complete", False), ("exit_code", 1), ("flaky", True),
                             ("attempts", 2), ("selected", 0), ("skipped", 1), ("retries", 1)]:
            with self.subTest(field=field, value=value):
                rows = copy.deepcopy(self.receipts)
                rows[1][field] = value
                self.assertFalse(self.report(rows)["ready"])
        self.assertFalse(self.report(self.receipts + [self.receipts[1]])["ready"])

    def test_pr_head_and_actual_checkout_are_separate_and_stale_head_blocks(self):
        for field in ["checkout_sha", "pr_head_sha"]:
            state = {**self.state, field: self.base}
            result = self.report(state=state)
            self.assertFalse(result["ready"])
            self.assertEqual(result[field if field == "checkout_sha" else "observed_pr_head_sha"], self.base)
        self.assertFalse(self.report(state={**self.state, "worktree_clean": False})["ready"])

    def test_publication_requires_real_release_record_and_resolved_candidate(self):
        state = {**self.state, "merge": {"merged": True, "merged_at": "2026-09-07T00:00:00Z",
            "head_sha": self.candidate, "merge_commit_sha": self.candidate},
            "release": {"tag_name": "v1.0.0"}}
        self.assertFalse(self.report(state=state)["phases"]["published"]["passed"])
        state["release"].update(id=1, draft=False, published_at="2026-09-07T00:00:00Z",
            html_url="https://example.invalid/releases/1", resolved_source_sha=self.candidate)
        self.assertTrue(self.report(state=state)["complete"])
        for field, value in [("resolved_source_sha", self.base), ("draft", True), ("published_at", None)]:
            altered = copy.deepcopy(state)
            altered["release"][field] = value
            self.assertFalse(self.report(state=altered)["phases"]["published"]["passed"])

    def test_malformed_manifest_overwrites_old_green_output(self):
        output = self.repo / "readiness.json"
        output.write_text('{"ready":true}')
        manifest = self.repo / "manifest.json"
        manifest.write_text(json.dumps({**self.manifest, "checks": []}))
        with contextlib.redirect_stderr(io.StringIO()):
            code = gate.main(["status", "--repo", str(self.repo), "--manifest", str(manifest),
                              "--state", "missing", "--receipts", "missing", "--output", str(output)])
        self.assertEqual(code, 1)
        self.assertFalse(json.loads(output.read_text())["ready"])


if __name__ == "__main__":
    unittest.main()
