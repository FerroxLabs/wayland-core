#!/usr/bin/env python3
"""Offline positive/negative controls for the original release input wiring."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

merge = load("merge", ".github/scripts/merge-stabilization-inputs.py")
native = load("native", ".github/scripts/collect-stabilization-native.py")
SHA = "a" * 40
TAG = "v1.2.3"
REPO = "FerroxLabs/wayland-core"


def write(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return {"path": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


class Collection(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.assets = self.root / "artifacts"
        self.assets.mkdir()
        for target in merge.gate.TARGETS:
            ext = "zip" if "windows" in target else "tar.gz"
            (self.assets / f"wayland-core-{TAG}-{target}.{ext}").write_text(target)
        self.sources = []
        for label, kind, job, run in (("ci", "tests", "ci-linux", "4"),
                                     ("mutants", "mutants", "collect-release-mutants", "5"),
                                     ("native", "native", "collect-release-native", "5")):
            folder = self.root / label
            folder.mkdir()
            data = {"source_sha": SHA, kind: [], "provenance": {"source_sha": SHA,
                    "repository": REPO, "run_id": run, "job": job}}
            if label == "ci":
                data[kind] = [self.row(folder, "packaged_driver_gate")]
                data["deferred_risks"] = [{"id": risk, "severity": "high", "disposition": "standing disposition; native evidence pending"}
                                          for risk in ("core#368", "core#410")]
            elif label == "mutants":
                for mutant in merge.gate.MUTANTS:
                    baseline = self.row(folder, mutant + "-baseline")
                    row = self.row(folder, mutant, failure=True)
                    row["baseline"] = baseline
                    row["patch"] = write(folder / (mutant + ".patch"), "fixture mutation")
                    data[kind].append(row)
            else:
                data[kind] = [self.row(folder, check, windows=True) for check in merge.gate.NATIVE_CHECKS]
            (folder / "index.json").write_text(json.dumps(data))
            self.sources.append((label, folder, kind, job, run))

    def row(self, folder, name, failure=False, windows=False):
        xml = '<testsuites><testsuite><testcase name="fixture">' + ('<failure>caught</failure>' if failure else '') + '</testcase></testsuite></testsuites>'
        junit = write(folder / (name + ".xml"), xml)
        proof = {"schema": "stabilization-command-1", "source": SHA, "complete": True,
                 "argv": ["cargo", "nextest", "run", "--no-tests=fail", "--retries", "0", name],
                 "diff": "fixture mutation" if failure else "", "exit_code": 100 if failure else 0,
                 "junit_sha256": junit["sha256"]}
        if windows:
            proof.update(platform="Windows", machine="AMD64", startup="wayland-core 1.2.3", startup_exit=0,
                         artifact_sha256=merge.gate.digest(self.assets / f"wayland-core-{TAG}-x86_64-pc-windows-msvc.zip"))
        return {"id": name, "proof": write(folder / (name + ".json"), json.dumps(proof)), "junit": junit}

    def run_merge(self):
        return merge.merge(self.sources, self.root / "merged", SHA, REPO, "5", "4", self.assets, TAG)

    def mutate_index(self, label, fn):
        path = self.root / label / "index.json"
        data = json.loads(path.read_text())
        fn(data)
        path.write_text(json.dumps(data))

    def test_full_original_contract_and_risks_survive(self):
        result = self.run_merge()
        self.assertEqual(len(result["mutants"]), 8)
        self.assertEqual(len(result["native"]), 4)
        self.assertIn("prior ACL nonpass retained", result["deferred_risks"][0]["disposition"])
        self.assertEqual(result["deferred_risks"][1]["disposition"], "standing disposition; native evidence pending")
        self.assertEqual(result["provenance"][0]["run_id"], "4")

    def test_stale_source_is_not_relabelled(self):
        self.mutate_index("mutants", lambda d: d.update(source_sha="b" * 40))
        with self.assertRaisesRegex(ValueError, "stale source"):
            self.run_merge()

    def test_foreign_producer_refused(self):
        self.mutate_index("native", lambda d: d["provenance"].update(run_id="9"))
        with self.assertRaisesRegex(ValueError, "provenance mismatch"):
            self.run_merge()

    def test_missing_mutant_refused(self):
        self.mutate_index("mutants", lambda d: d["mutants"].pop())
        with self.assertRaisesRegex(ValueError, "missing mutants"):
            self.run_merge()

    def test_wrong_archive_refused(self):
        (self.assets / f"wayland-core-{TAG}-x86_64-pc-windows-msvc.zip").write_text("replacement")
        with self.assertRaisesRegex(ValueError, "candidate archive"):
            self.run_merge()

    def test_tampered_raw_file_refused(self):
        (self.root / "ci/packaged_driver_gate.xml").write_text("tampered")
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            self.run_merge()

    def test_nonwindows_collection_refused(self):
        with patch.object(native.platform, "system", return_value="Linux"):
            with self.assertRaisesRegex(ValueError, "Windows x64"):
                native.collect(self.root / "new", self.assets / "x", SHA)

    def test_any_current_native_failure_is_blocking(self):
        passing = {"selected": 18, "passed": 18, "failed": 0, "skipped": 0, "retries": 0}
        native.require_passing(0, passing, 18)
        for code, changed in ((100, {"passed": 17, "failed": 1}),
                              (0, {"selected": 17}), (0, {"skipped": 1}),
                              (0, {"retries": 1}), (101, {})):
            with self.assertRaisesRegex(ValueError, "native checks failed"):
                native.require_passing(code, {**passing, **changed}, 18)
        self.assertNotIn('row["disposition"]', (ROOT / ".github/scripts/collect-stabilization-native.py").read_text())

    def test_native_selection_retains_repair_regressions(self):
        args = native.selection("wcore-sandbox", "live_fs_acl")
        self.assertIn("live_fs_acl", args)
        self.assertIn("--lib", args)
        for name in native.LEASE_TESTS:
            self.assertIn(name, args[-1])
        self.assertEqual(dict((check, count) for _, check, count in native.CHECKS)["live_fs_acl"], 18)

    def test_workflow_requires_same_run_producers_and_keeps_ci_authentication(self):
        text = (ROOT / ".github/workflows/release.yml").read_text()
        produce = text.split("  produce-release-evidence:\n", 1)[1].split("  promote-release:\n", 1)[0]
        self.assertIn("post-tag-smoke, collect-release-mutants, collect-release-native]", produce)
        self.assertIn('run["conclusion"] == "success"', produce)
        self.assertIn('run["head_sha"] == sys.argv[1]', produce)
        self.assertIn('run["event"] in ("push", "workflow_dispatch")', produce)
        self.assertIn("--inputs merged-evidence/index.json", produce)
        self.assertIn("name: release-native-raw", produce)
        self.assertIn("name: release-mutants-raw", produce)
        collectors = text.split("  collect-release-mutants:\n", 1)[1].split("  produce-release-evidence:\n", 1)[0]
        self.assertNotIn("GH_TOKEN:", collectors)
        self.assertNotIn("continue-on-error", collectors)
        self.assertEqual(collectors.count("needs: [prepare-release, github-release]"), 2)


if __name__ == "__main__":
    unittest.main()
