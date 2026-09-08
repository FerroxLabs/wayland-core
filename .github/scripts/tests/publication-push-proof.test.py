#!/usr/bin/env python3
"""The publication shortcut must carry complete source-bound push coverage."""
import copy
import importlib.util
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location("proof", ROOT / ".github/scripts/publication-push-proof.py")
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)
REPO = "FerroxLabs/wayland-core"
HEAD = "a" * 40
BASE = "b" * 40


class Proof(unittest.TestCase):
    def setUp(self):
        self.pr = {"state": "open", "head": {"repo": {"full_name": REPO}, "ref": proof.PUBLICATION_HEAD, "sha": HEAD},
                   "base": {"ref": "main", "sha": BASE}}
        self.run = {"id": 10, "repository": {"full_name": REPO}, "head_repository": {"full_name": REPO},
                    "event": "push", "head_sha": HEAD, "head_branch": "integ/release-fixture",
                    "status": "completed", "conclusion": "success"}
        self.jobs = [{"name": name, "status": "completed", "conclusion": "success"} for name in proof.REQUIRED_JOBS]
        self.artifacts = [{"name": name, "expired": False, "workflow_run": {"id": 10, "head_sha": HEAD}}
                          for name in ("nextest-junit-linux-containerized", "nextest-junit-macos-latest",
                                       "nextest-junit-Array", "stabilization-raw-" + HEAD)]

    def test_real_completed_push_with_every_producer_is_accepted(self):
        proof.validate_pr(self.pr, REPO, HEAD, BASE)
        proof.validate_run(self.run, self.jobs, self.artifacts, REPO, HEAD)

    def test_head_base_and_foreign_pr_cannot_inherit(self):
        for kind in ("head", "base"):
            pr = copy.deepcopy(self.pr)
            pr[kind]["sha"] = "c" * 40
            with self.assertRaises(ValueError):
                proof.validate_pr(pr, REPO, HEAD, BASE)
        pr = copy.deepcopy(self.pr)
        pr["head"]["repo"]["full_name"] = "foreign/fork"
        with self.assertRaises(ValueError):
            proof.validate_pr(pr, REPO, HEAD, BASE)

    def test_wrong_event_source_or_integration_branch_refused(self):
        for changed in ({"event": "pull_request"}, {"head_sha": "c" * 40},
                        {"head_branch": "lane/unrelated"}, {"conclusion": "failure"}):
            with self.assertRaises(ValueError):
                proof.validate_run({**self.run, **changed}, self.jobs, self.artifacts, REPO, HEAD)

    def test_missing_skipped_or_failed_producer_refused(self):
        for removed in proof.REQUIRED_JOBS:
            with self.assertRaises(ValueError):
                proof.validate_run(self.run, [job for job in self.jobs if job["name"] != removed], self.artifacts, REPO, HEAD)
        for status in ("failure", "skipped", "cancelled"):
            jobs = copy.deepcopy(self.jobs)
            jobs[0]["conclusion"] = status
            with self.assertRaises(ValueError):
                proof.validate_run(self.run, jobs, self.artifacts, REPO, HEAD)

    def test_missing_expired_foreign_artifacts_refused(self):
        with self.assertRaises(ValueError):
            proof.validate_run(self.run, self.jobs, self.artifacts[:-1], REPO, HEAD)
        for changed in ({"expired": True}, {"workflow_run": {"id": 11, "head_sha": HEAD}}):
            artifacts = copy.deepcopy(self.artifacts)
            artifacts[0].update(changed)
            with self.assertRaises(ValueError):
                proof.validate_run(self.run, self.jobs, artifacts, REPO, HEAD)

    def test_scoped_workflow_preserves_single_full_push_and_report_guard(self):
        source = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("publication_pr: ${{ github.event_name == 'pull_request' && github.event.pull_request.head.repo.full_name == github.repository && github.head_ref == 'stabilize/20260905-core' }}", source)
        for name in ("ci", "ci-windows-hosted", "ci-linux", "eval-gate-linux", "build", "build-darwin-selfhosted", "all-features-check"):
            block = re.search(r"(?ms)^  " + name + r":\n(.*?)(?=^  [a-z][a-z0-9-]*:|\Z)", source)[1]
            self.assertIn("needs.admission.outputs.publication_pr != 'true'", block)
        report = source.split("  report:\n", 1)[1]
        self.assertIn("all-features-check, publication-push-proof]", report)
        self.assertIn("run: bash .github/scripts/assert-no-dependency-failed.sh", report)
        self.assertIn("run: bash .github/scripts/assert-test-evidence.sh", report)
        self.assertIn("run-id: ${{ needs.publication-push-proof.outputs.run_id || github.run_id }}", report)
        self.assertIn("- 'integ/**'", source)

    def test_native_work_overlaps_only_its_own_bounded_build(self):
        source = (ROOT / ".github/workflows/ci.yml").read_text()
        for name, prior in (("ci", 120), ("ci-windows-hosted", 180)):
            block = re.search(r"(?ms)^  " + name + r":\n(.*?)(?=^  [a-z][a-z0-9-]*:|\Z)", source)[1]
            self.assertIn("    needs: admission\n", block)
            self.assertNotIn("needs.build.outputs", block)
            self.assertIn('--producer-job "Build ($native_target)"', block)
            self.assertIn("--wait-seconds 4500", block)
            self.assertIn(f"timeout-minutes: {75 + prior}", block)

    def test_attribution_evaluator_preserves_push_and_delegates_only_publication_pr(self):
        spec = importlib.util.spec_from_file_location("attribution", ROOT / "scripts/check-windows-attribution.py")
        attribution = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(attribution)
        atom = "needs.admission.outputs.publication_pr != 'true'"
        context = {"event_name": "push", "ref_name": "integ/release-fixture", "commit_messages": ""}
        self.assertTrue(attribution.eval_gha_if(atom, context))
        self.assertFalse(attribution.eval_gha_if(atom, {**context, "event_name": "pull_request", "publication_pr": True}))
        self.assertTrue(attribution.eval_gha_if(atom, {**context, "event_name": "pull_request", "publication_pr": False}))
        self.assertIsNone(attribution.eval_gha_if(atom, {**context, "event_name": "pull_request"}))

    def test_source_proof_includes_base_ancestry_and_exact_merge_tree(self):
        source = (ROOT / ".github/scripts/publication-push-proof.py").read_text()
        self.assertIn('["git", "merge-base", "--is-ancestor", base, head]', source)
        self.assertIn('"HEAD^{tree}"', source)
        self.assertIn('head + "^{tree}"', source)
        self.assertEqual(source.count('validate_pr(api('), 2)
        self.assertNotIn('workflow", "run', source)
        self.assertNotIn('run", "rerun', source)


if __name__ == "__main__":
    unittest.main()
